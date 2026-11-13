//! The console's log, against the stand-in: the password reaches the console's output once and no
//! log line, nor does a wrong one tried, and the log names the certificate's fingerprint. Alone in
//! its binary, so the process-wide subscriber that captures the log sees no other test.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::StandIn;
use common::console::{Running, console_config, printed_password, seed, until_started};

#[tokio::test]
async fn the_password_is_never_logged() {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&logs);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || SharedLog(Arc::clone(&writer)))
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let password = printed_password(&running.printed);
    assert_eq!(running.login("tried-and-wrong").await.0.status, 401);
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    running.stop_after(&stand_in).await.unwrap();
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("login refused: wrong password") && logs.contains("logged in"), "{logs}");
    assert!(logs.contains("console listening") && logs.contains("fingerprint="), "{logs}");
    assert!(!logs.contains(&password), "the password was logged");
    assert!(!logs.contains("tried-and-wrong"), "a tried password was logged");
}

/// A log writer into a shared buffer.
struct SharedLog(Arc<Mutex<Vec<u8>>>);

impl Write for SharedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
