//! A native global shortcut on a dedicated message thread; no input hook.
use std::thread::{self, JoinHandle};
use windows::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
            RegisterHotKey, UnregisterHotKey,
        },
        WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
        },
    },
};

#[derive(Clone, Copy)]
pub enum ShortcutAction {
    Start,
    Stop,
    Toggle,
}
fn parse(text: &str) -> Result<Option<(u32, u32)>, String> {
    if text.trim().is_empty() || text.trim().eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let mut modifiers = 0;
    let mut key = None;
    for part in text.split('+').map(|part| part.trim().to_ascii_uppercase()) {
        let modifier = match part.as_str() {
            "CTRL" | "CONTROL" => Some(MOD_CONTROL.0),
            "SHIFT" => Some(MOD_SHIFT.0),
            "ALT" => Some(MOD_ALT.0),
            "WIN" | "WINDOWS" => Some(MOD_WIN.0),
            _ => None,
        };
        if let Some(modifier) = modifier {
            modifiers |= modifier;
            continue;
        }
        let code = match part.as_str() {
            "SPACE" => 0x20,
            "PAUSE" => 0x13,
            "INSERT" => 0x2d,
            "DELETE" => 0x2e,
            "HOME" => 0x24,
            "END" => 0x23,
            "PAGEUP" => 0x21,
            "PAGEDOWN" => 0x22,
            "ESC" | "ESCAPE" => 0x1b,
            _ if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() => {
                u32::from(part.as_bytes()[0])
            }
            _ if part.starts_with('F') => part[1..]
                .parse::<u32>()
                .ok()
                .filter(|number| (1..=24).contains(number))
                .map(|number| 0x70 + number - 1)
                .ok_or_else(|| format!("Unknown shortcut key: {part}"))?,
            _ => {
                return Err(format!(
                    "Unknown shortcut key: {part}. Use a letter, number, or F1–F24."
                ));
            }
        };
        if key.replace(code).is_some() {
            return Err("A shortcut needs one main key.".into());
        }
    }
    let key = key.ok_or("A shortcut needs a main key, such as F9.")?;
    if modifiers == 0 && !(0x70..=0x87).contains(&key) && key != 0x13 {
        return Err("Use Ctrl, Alt, Shift, or Win with a letter or navigation key.".into());
    }
    Ok(Some((modifiers, key)))
}

pub struct RecordingShortcut {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
}
impl RecordingShortcut {
    pub fn validate(start: &str, stop: &str) -> Result<(), String> {
        parse(start)?;
        parse(stop)?;
        Ok(())
    }
    pub fn register(
        start: &str,
        stop: &str,
        pressed: impl Fn(ShortcutAction) + Send + 'static,
    ) -> Result<Self, String> {
        let start = parse(start)?;
        let stop = parse(stop)?;
        let mut bindings = Vec::new();
        if let Some(key) = start.filter(|_| start == stop) {
            bindings.push((1, key, ShortcutAction::Toggle));
        } else {
            if let Some(key) = start {
                bindings.push((1, key, ShortcutAction::Start));
            }
            if let Some(key) = stop {
                bindings.push((2, key, ShortcutAction::Stop));
            }
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("fastrecorder-shortcut".into())
            .spawn(move || unsafe {
                let mut message = MSG::default();
                // Create the message queue before publishing its thread ID.
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                let mut registered = Vec::new();
                for (id, (modifiers, key), _) in &bindings {
                    if let Err(error) = RegisterHotKey(
                        None,
                        *id,
                        HOT_KEY_MODIFIERS(*modifiers | MOD_NOREPEAT.0),
                        *key,
                    ) {
                        for id in registered {
                            let _ = UnregisterHotKey(None, id);
                        }
                        let _ = sender.send(Err(format!(
                            "Shortcut is unavailable or already in use: {error}"
                        )));
                        return;
                    }
                    registered.push(*id);
                }
                if sender.send(Ok(GetCurrentThreadId())).is_err() {
                    for id in registered {
                        let _ = UnregisterHotKey(None, id);
                    }
                    return;
                }
                while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    if message.message == WM_HOTKEY
                        && let Some((_, _, action)) = bindings
                            .iter()
                            .find(|(id, _, _)| message.wParam.0 == *id as usize)
                    {
                        pressed(*action);
                    }
                }
                for id in registered {
                    let _ = UnregisterHotKey(None, id);
                }
            })
            .map_err(|e| e.to_string())?;
        match receiver.recv().map_err(|e| e.to_string())? {
            Ok(thread_id) => Ok(Self {
                thread_id,
                worker: Some(worker),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
}
impl Drop for RecordingShortcut {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(
                self.thread_id,
                WM_QUIT,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
