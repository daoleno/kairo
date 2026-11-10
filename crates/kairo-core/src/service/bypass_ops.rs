//! The service's look for tools that go around the proxy (see `bypass`):
//! every ten seconds, only while the core runs with the system proxy on and
//! TUN off, and only reading the socket table when some process looks like an
//! AI tool.

use std::time::Duration;

use super::{Service, unix};
use crate::bypass::{self, Bypass};
use crate::settings::Mode;

impl Service {
    /// Forgets what was seen (the core stopped, or TUN took over).
    pub(super) fn clear_bypass(&self) {
        self.0.state.send_if_modified(|s| {
            let had = !s.bypass.is_empty();
            s.bypass.clear();
            had
        });
    }

    /// Every ten seconds.
    pub(super) async fn bypass_loop(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.tick().await;
        loop {
            tick.tick().await;
            self.bypass_poll().await;
        }
    }

    /// One look. Without the system proxy, or with TUN (which sees every
    /// connection) or the Direct route (the person sent everything out),
    /// there is nothing to find, and the list is emptied.
    async fn bypass_poll(&self) {
        let st = self.snapshot();
        let watching = st.core.is_running()
            && st.settings.system_proxy
            && !st.settings.tun
            && st.settings.mode != Mode::Direct;
        if !watching {
            self.clear_bypass();
            return;
        }
        let port = st.settings.mixed_port;
        // Kairo itself and its core are not tools.
        let mut exclude = vec![std::process::id()];
        if let Some(core) = std::fs::read_to_string(self.0.paths.core_pid())
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
        {
            exclude.push(core);
        }
        let now = unix();
        let found = tokio::task::spawn_blocking(move || bypass::scan(port, &exclude, now))
            .await
            .unwrap_or_default();
        // It may have changed while the commands ran.
        let still = {
            let s = self.0.state.borrow();
            s.core.is_running() && !s.settings.tun
        };
        if !still {
            self.clear_bypass();
            return;
        }
        self.0.state.send_if_modified(|s| {
            let merged: Vec<Bypass> = bypass::merge_since(&s.bypass, found);
            if merged == s.bypass {
                return false;
            }
            s.bypass = merged;
            true
        });
    }

    /// What to paste in a terminal so command-line tools use the proxy.
    pub fn terminal_env(&self) -> String {
        bypass::terminal_env(self.settings().mixed_port)
    }
}
