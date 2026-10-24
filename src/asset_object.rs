//! A library item as a GObject, so it can live in a `gio::ListStore` and be
//! shown by `gtk::GridView`. `favorite` is a property so cells update when
//! it changes.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;

use crate::library::Asset;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::AssetObject)]
    pub struct AssetObject {
        pub asset: RefCell<Option<Asset>>,
        #[property(get, set)]
        pub favorite: Cell<bool>,
        /// Bumped when what the item looks like changes (an edit), so cells redraw.
        #[property(get, set)]
        pub revision: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AssetObject {
        const NAME: &'static str = "CloudPhotosAsset";
        type Type = super::AssetObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for AssetObject {}
}

glib::wrapper! {
    pub struct AssetObject(ObjectSubclass<imp::AssetObject>);
}

impl AssetObject {
    pub fn new(asset: Asset) -> Self {
        let object: Self = glib::Object::builder().property("favorite", asset.favorite).build();
        object.imp().asset.replace(Some(asset));
        object
    }

    /// A copy of the asset data.
    pub fn asset(&self) -> Asset {
        self.imp().asset.borrow().clone().expect("asset set at construction")
    }

    pub fn id(&self) -> String {
        self.imp().asset.borrow().as_ref().map(|a| a.id.clone()).unwrap_or_default()
    }

    pub fn with<R>(&self, f: impl FnOnce(&Asset) -> R) -> R {
        f(self.imp().asset.borrow().as_ref().expect("asset set at construction"))
    }

    pub fn update(&self, f: impl FnOnce(&mut Asset)) {
        let favorite = {
            let mut asset = self.imp().asset.borrow_mut();
            let asset = asset.as_mut().expect("asset set at construction");
            f(asset);
            asset.favorite
        };
        if favorite != self.favorite() {
            self.set_favorite(favorite);
        }
    }
}
