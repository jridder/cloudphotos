//! Video playback on Windows: a GtkMediaStream that plays through
//! GStreamer's playbin into gtk4paintablesink. GTK's own GStreamer media
//! (gtk::MediaFile) hands over D3D12 textures that come out blank there.

use std::{cell::RefCell, time::Duration};

use gst::prelude::*;
use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::*};

mod imp {
    use std::cell::OnceCell;

    use super::*;

    #[derive(Default)]
    pub struct Player {
        pub playbin: OnceCell<gst::Element>,
        pub paintable: OnceCell<gdk::Paintable>,
        pub bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
        pub ticker: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Player {
        const NAME: &'static str = "CloudPhotosPlayer";
        type Type = super::Player;
        type ParentType = gtk::MediaStream;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for Player {
        fn dispose(&self) {
            if let Some(ticker) = self.ticker.take() {
                ticker.remove();
            }
            self.bus_watch.take();
            if let Some(playbin) = self.playbin.get() {
                let _ = playbin.set_state(gst::State::Null);
            }
        }
    }

    impl MediaStreamImpl for Player {
        fn play(&self) -> bool {
            self.playbin.get().is_some_and(|p| p.set_state(gst::State::Playing).is_ok())
        }

        fn pause(&self) {
            if let Some(playbin) = self.playbin.get() {
                let _ = playbin.set_state(gst::State::Paused);
            }
        }

        fn seek(&self, timestamp: i64) {
            let position = gst::ClockTime::from_useconds(timestamp.max(0) as u64);
            let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
            if self.playbin.get().is_some_and(|p| p.seek_simple(flags, position).is_ok()) {
                self.obj().seek_success();
            } else {
                self.obj().seek_failed();
            }
        }

        fn update_audio(&self, muted: bool, volume: f64) {
            if let Some(playbin) = self.playbin.get() {
                playbin.set_property("mute", muted);
                playbin.set_property("volume", volume);
            }
        }
    }

    impl PaintableImpl for Player {
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some(paintable) = self.paintable.get() {
                paintable.snapshot(snapshot, width, height);
            }
        }

        fn current_image(&self) -> gdk::Paintable {
            match self.paintable.get() {
                Some(paintable) => paintable.current_image(),
                None => gdk::Paintable::new_empty(0, 0),
            }
        }

        fn intrinsic_width(&self) -> i32 {
            self.paintable.get().map_or(0, |p| p.intrinsic_width())
        }

        fn intrinsic_height(&self) -> i32 {
            self.paintable.get().map_or(0, |p| p.intrinsic_height())
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            self.paintable.get().map_or(0.0, |p| p.intrinsic_aspect_ratio())
        }
    }
}

glib::wrapper! {
    pub struct Player(ObjectSubclass<imp::Player>)
        @extends gtk::MediaStream,
        @implements gdk::Paintable;
}

impl Player {
    pub fn for_file(file: &gio::File) -> Self {
        let player: Self = glib::Object::new();
        if let Err(e) = player.build(file) {
            player.set_error(glib::Error::new(gst::CoreError::MissingPlugin, &format!("Couldn't play it: {e}")));
        }
        player
    }

    fn build(&self, file: &gio::File) -> Result<(), glib::BoolError> {
        gst::init().map_err(|e| glib::bool_error!("{e}"))?;
        let sink = gst::ElementFactory::make("gtk4paintablesink").build()?;
        let playbin = gst::ElementFactory::make("playbin").property("uri", file.uri()).property("video-sink", &sink).build()?;
        let imp = self.imp();

        let paintable = sink.property::<gdk::Paintable>("paintable");
        let weak = self.downgrade();
        paintable.connect_invalidate_contents(move |_| {
            if let Some(player) = weak.upgrade() {
                player.invalidate_contents();
            }
        });
        let weak = self.downgrade();
        paintable.connect_invalidate_size(move |_| {
            if let Some(player) = weak.upgrade() {
                player.invalidate_size();
            }
        });
        let _ = imp.paintable.set(paintable);

        let weak = self.downgrade();
        let bus = playbin.bus().ok_or_else(|| glib::bool_error!("playbin has no bus"))?;
        let watch = bus.add_watch_local(move |_, message| {
            let Some(player) = weak.upgrade() else { return glib::ControlFlow::Break };
            player.on_message(message);
            glib::ControlFlow::Continue
        })?;
        imp.bus_watch.replace(Some(watch));

        // The position, for the playback controls.
        let weak = self.downgrade();
        let ticker = glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(player) = weak.upgrade() else { return glib::ControlFlow::Break };
            if player.is_playing() {
                if let Some(position) = player.imp().playbin.get().and_then(|p| p.query_position::<gst::ClockTime>()) {
                    player.update(position.useconds() as i64);
                }
            }
            glib::ControlFlow::Continue
        });
        imp.ticker.replace(Some(ticker));

        // Preroll; prepared once the first frame is ready (see on_message).
        playbin.set_state(gst::State::Paused).map_err(|e| glib::bool_error!("{e}"))?;
        let _ = imp.playbin.set(playbin);
        Ok(())
    }

    fn on_message(&self, message: &gst::Message) {
        let Some(playbin) = self.imp().playbin.get() else { return };
        match message.view() {
            gst::MessageView::AsyncDone(_) if !self.is_prepared() => {
                let duration = playbin.query_duration::<gst::ClockTime>().map_or(0, |d| d.useconds() as i64);
                let mut seeking = gst::query::Seeking::new(gst::Format::Time);
                let seekable = playbin.query(&mut seeking) && seeking.result().0;
                let (audio, video) = (playbin.property::<i32>("n-audio") > 0, playbin.property::<i32>("n-video") > 0);
                self.stream_prepared(audio, video, seekable, duration);
            }
            gst::MessageView::Eos(_) => self.stream_ended(),
            gst::MessageView::Error(e) => self.set_error(e.error()),
            _ => {}
        }
    }
}
