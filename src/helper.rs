//! The Python helper process (helper/photos_helper.py): the only part of
//! Cloud Photos that talks to Apple, because sign-in and the CloudKit API
//! come from pyicloud. Everything it sends is JSON lines; see the helper's
//! docstring for the methods.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{ChildStdin, Command, Stdio},
    rc::Rc,
};

use gtk::glib;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct HelperError {
    pub code: String,
    pub message: String,
}

impl HelperError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

type Reply = async_channel::Sender<Result<Value, HelperError>>;

pub struct Helper {
    stdin: RefCell<Option<ChildStdin>>,
    pending: Rc<RefCell<HashMap<u64, Reply>>>,
    next_id: Cell<u64>,
}

/// Where the helper script and its Python live: overridable for tests and
/// development, otherwise the Flatpak's bundled copy.
fn helper_command() -> Command {
    let script = std::env::var_os("CLOUDPHOTOS_HELPER").map(PathBuf::from).unwrap_or_else(|| {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helper/photos_helper.py");
        if std::path::Path::new("/.flatpak-info").exists() || !dev.exists() {
            PathBuf::from("/app/libexec/cloudphotos/photos_helper.py")
        } else {
            dev
        }
    });
    let python = std::env::var_os("CLOUDPHOTOS_PYTHON").map(PathBuf::from).unwrap_or_else(|| {
        // Outside the Flatpak, pyicloud comes from icloud-linux's venv.
        let venv = glib::home_dir().join(".local/share/icloud-linux/.venv/bin/python");
        if !std::path::Path::new("/.flatpak-info").exists() && venv.exists() { venv } else { "python3".into() }
    });
    let mut cmd = Command::new(python);
    cmd.arg("-u").arg(script);
    if let Some(fake) = std::env::var_os("CLOUDPHOTOS_FAKE") {
        cmd.arg("--fake").arg(fake);
    }
    cmd
}

impl Helper {
    /// Start the helper; `on_event` gets every streamed `{"event": ...}` message.
    pub fn spawn(on_event: impl Fn(Value) + 'static) -> Result<Rc<Self>, HelperError> {
        let mut child = helper_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| HelperError::new("helper_missing", format!("Couldn't start the iCloud helper: {e}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let helper = Rc::new(Helper {
            stdin: RefCell::new(child.stdin.take()),
            pending: Rc::new(RefCell::new(HashMap::new())),
            next_id: Cell::new(1),
        });

        let (tx, rx) = async_channel::unbounded::<Option<String>>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send_blocking(Some(line)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send_blocking(None);
            let _ = child.wait();
        });

        let pending = helper.pending.clone();
        glib::spawn_future_local(async move {
            while let Ok(Some(line)) = rx.recv().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                if message.get("event").is_some() {
                    on_event(message);
                    continue;
                }
                let Some(id) = message.get("id").and_then(Value::as_u64) else { continue };
                let Some(reply) = pending.borrow_mut().remove(&id) else { continue };
                let result = match message.get("error") {
                    Some(err) => Err(HelperError::new(
                        err.get("code").and_then(Value::as_str).unwrap_or("failed"),
                        err.get("message").and_then(Value::as_str).unwrap_or("Something went wrong."),
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = reply.send(result).await;
            }
            for (_, reply) in pending.borrow_mut().drain() {
                let _ = reply.try_send(Err(HelperError::new("helper_exited", "The iCloud helper stopped.")));
            }
        });
        Ok(helper)
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, HelperError> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (tx, rx) = async_channel::bounded(1);
        self.pending.borrow_mut().insert(id, tx);
        let line = json!({"id": id, "method": method, "params": params}).to_string() + "\n";
        let written = self
            .stdin
            .borrow_mut()
            .as_mut()
            .map(|stdin| stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()));
        if !matches!(written, Some(Ok(()))) {
            self.pending.borrow_mut().remove(&id);
            return Err(HelperError::new("helper_exited", "The iCloud helper stopped."));
        }
        rx.recv().await.unwrap_or_else(|_| Err(HelperError::new("helper_exited", "The iCloud helper stopped.")))
    }
}
