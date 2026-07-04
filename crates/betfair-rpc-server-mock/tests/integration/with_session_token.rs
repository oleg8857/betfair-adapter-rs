use betfair_adapter::SessionToken;
use betfair_rpc_server_mock::{BOT_LOGIN_URL, SESSION_TOKEN, Server};
use betfair_types::types::sports_aping::{ExecutionReportStatus, MarketId, cancel_orders};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

/// Building a client from an already-obtained session token must reuse that
/// token (proven by the authenticated request succeeding, since the mock
/// requires `X-Authentication: SESSION_TOKEN`) and must NOT perform a bot login.
#[rstest]
#[test_log::test(tokio::test)]
async fn with_session_token_reuses_token_without_bot_login() {
    let server = Server::new().await;

    // Setup: an authenticated RPC that only matches when the session token header is set.
    let response = json!({
        "status": "SUCCESS",
        "marketId": "1.210878100",
        "instructionReports": []
    });
    server
        .mock_authenticated_rpc_from_json::<cancel_orders::Parameters>(response)
        .expect(1)
        .mount(&server.bf_api_mock_server)
        .await;

    // Action: build the authenticated client straight from the token, no `authenticate()` call.
    let unauth = server.client().await;
    let client = unauth
        .with_session_token(SessionToken::new(SESSION_TOKEN.to_owned()))
        .unwrap();
    let result = client
        .send_request(cancel_orders::Parameters {
            market_id: Some(MarketId::new("1.210878100")),
            instructions: None,
            customer_ref: None,
        })
        .await
        .unwrap();

    // Assert: the request succeeded, i.e. the token was applied as `X-Authentication`.
    assert_eq!(result.status, ExecutionReportStatus::Success);

    // Assert: no bot login happened (no request ever hit the cert-login endpoint).
    let requests = server.bf_api_mock_server.received_requests().await.unwrap();
    let bot_login_count = requests
        .iter()
        .filter(|req| req.url.path() == BOT_LOGIN_URL)
        .count();
    assert_eq!(bot_login_count, 0);
}
