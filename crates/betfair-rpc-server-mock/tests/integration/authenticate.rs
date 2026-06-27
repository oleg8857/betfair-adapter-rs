use std::time::Duration;

use betfair_adapter::jurisdiction::CustomUrl;
use betfair_adapter::{ApiError, BetfairRpcClient};
use betfair_rpc_server_mock::{BOT_LOGIN_URL, SESSION_TOKEN, Server};
use betfair_types::bot_login::LoginError;
use serde_json::json;
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Happy path: valid credentials succeed on the first attempt and yield the
/// session token from the (default) login mock.
#[test_log::test(tokio::test)]
async fn authenticate_success_returns_session_token() {
    let server = Server::new().await;
    let client = server.client().await;

    let (client, _keep_alive) = timeout(Duration::from_secs(5), client.authenticate())
        .await
        .expect("must not hang")
        .expect("authentication must succeed");

    assert_eq!(client.session_token().0.expose_secret(), SESSION_TOKEN);
}

/// Happy path with recovery: a transient failure on the first attempt is
/// retried, and the next attempt falls through to the default success mock.
#[test_log::test(tokio::test)]
async fn authenticate_transient_error_then_succeeds() {
    let server = Server::new().await;

    // Higher-priority mock that answers only the first request with a 500 and an
    // empty (non-JSON) body, which surfaces as a transient error. It is consumed
    // after one response, so the retry falls through to the default login mock.
    Mock::given(method("POST"))
        .and(path(BOT_LOGIN_URL))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .up_to_n_times(1)
        .named("Transient login failure")
        .expect(1)
        .mount(&server.bf_api_mock_server)
        .await;

    let client = server.client().await;
    let (client, _keep_alive) = timeout(Duration::from_secs(10), client.authenticate())
        .await
        .expect("must not hang")
        .expect("authentication must succeed after retry");

    assert_eq!(client.session_token().0.expose_secret(), SESSION_TOKEN);
}

/// Invalid password is a permanent error: return immediately, no retries.
#[test_log::test(tokio::test)]
async fn authenticate_invalid_password_returns_immediately() {
    let server = Server::new().await;

    // Mount a failing mock with priority 1 over the default success login mock
    // (it overrides the default). `.expect(1)` also verifies there are no retries.
    Mock::given(method("POST"))
        .and(path(BOT_LOGIN_URL))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"loginStatus": "INVALID_USERNAME_OR_PASSWORD"})),
        )
        .with_priority(1)
        .named("Login failure")
        .expect(1)
        .mount(&server.bf_api_mock_server)
        .await;

    let client = server.client().await;
    let res = timeout(Duration::from_secs(5), client.authenticate())
        .await
        .expect("must not hang");

    assert!(matches!(
        res,
        Err(ApiError::BotLoginError(
            LoginError::InvalidUsernameOrPassword
        ))
    ));
}

/// Network error is transient: retry with backoff, then return the last error
/// once the attempts are exhausted.
#[test_log::test(tokio::test)]
async fn authenticate_network_error_retries_then_returns() {
    // Needed for `secrets_provider()` with the identity certificate.
    let server = Server::new().await;

    let secrets = server.secrets_provider();
    let mut config = server.betfair_config(secrets);
    // Reserve a loopback port and free it immediately, so every connection
    // attempt is refused right away (more robust than a fixed magic port).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    config.bot_login = CustomUrl::new(format!("http://{addr}/cert-login/").parse().unwrap());
    let client = BetfairRpcClient::new_with_config(config).unwrap();

    // Headroom for the backoff pauses (~1+2+4 s).
    let start = std::time::Instant::now();
    let res = timeout(Duration::from_secs(15), client.authenticate())
        .await
        .expect("must not hang");
    let elapsed = start.elapsed();

    assert!(matches!(res, Err(ApiError::ReqwestError(_))));
    // Pin that retries actually happened: an immediate return (e.g. if the
    // network error were misclassified as permanent) would skip the backoff.
    assert!(
        elapsed >= Duration::from_secs(3),
        "expected backoff retries, but authenticate() returned in {elapsed:?}"
    );
}
