use std::io;
use std::sync::{Arc, Mutex};

use betfair_rpc_server_mock::{SESSION_TOKEN, Server};
use rstest::rstest;
use tracing_subscriber::fmt::MakeWriter;

/// A `MakeWriter` that appends all emitted log bytes into a shared buffer,
/// so a test can inspect what `tracing` actually wrote.
#[derive(Clone)]
struct BufferWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[rstest]
#[test_log::test(tokio::test)]
async fn keep_alive() {
    let server = Server::new().await;

    // Setup
    server
        .mock_keep_alive()
        .mount(&server.bf_api_mock_server)
        .await;

    // Action
    let client = server.client().await;
    let (client, _) = client.authenticate().await.unwrap();
    let ka = client.keep_alive().unwrap();
    ka.execute()
        .await
        .unwrap()
        .json_err()
        .await
        .unwrap()
        .unwrap();
}

#[rstest]
#[test_log::test(tokio::test)]
async fn keep_alive_does_not_leak_session_token() {
    let server = Server::new().await;

    server
        .mock_keep_alive()
        .mount(&server.bf_api_mock_server)
        .await;

    let client = server.client().await;
    let (client, _) = client.authenticate().await.unwrap();

    // Capture INFO-level logs emitted only while building the keep-alive request.
    // `keep_alive()` is synchronous, so a thread-local subscriber covers it fully.
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(BufferWriter(Arc::clone(&buffer)))
        .with_max_level(tracing::Level::INFO)
        .finish();

    let ka = tracing::subscriber::with_default(subscriber, || client.keep_alive().unwrap());

    let logs = String::from_utf8(buffer.lock().expect("log buffer poisoned").clone())
        .expect("logs are valid UTF-8");

    assert!(
        !logs.contains(SESSION_TOKEN),
        "session token leaked into tracing logs:\n{logs}"
    );

    // Sanity-check the request still works end to end.
    ka.execute()
        .await
        .unwrap()
        .json_err()
        .await
        .unwrap()
        .unwrap();
}
