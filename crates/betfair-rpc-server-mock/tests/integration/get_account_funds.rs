use betfair_adapter::ApiError;
use betfair_rpc_server_mock::{Server, rpc_path};
use betfair_types::types::account_aping::{ErrorCode, get_account_funds};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;
use wiremock::matchers::path;

/// Betfair wraps REST exceptions in a SOAP-like fault envelope, so the real
/// `errorCode` lives at `detail.<exceptionname>.errorCode`, not at the root.
/// This is the exact body returned by Account API `getAccountFunds` when the
/// session token is invalid. The client must unwrap the envelope and surface
/// `INVALID_SESSION_INFORMATION` instead of losing it to `None`.
#[rstest]
#[test_log::test(tokio::test)]
async fn get_account_funds_invalid_session_is_unwrapped() {
    let server = Server::new().await;

    let fault_envelope = json!({
        "faultcode": "Client",
        "faultstring": "AANGX-0002",
        "detail": {
            "exceptionname": "AccountAPINGException",
            "AccountAPINGException": {
                "requestUUID": "null",
                "errorCode": "INVALID_SESSION_INFORMATION",
                "errorDetails": ""
            }
        }
    });
    server
        .mock_error(
            "POST",
            path(rpc_path::<get_account_funds::Parameters>()),
            &rpc_path::<get_account_funds::Parameters>(),
            true,
            fault_envelope,
        )
        .expect(1)
        .mount(&server.bf_api_mock_server)
        .await;

    // Action
    let client = server.client().await;
    let (client, _) = client.authenticate().await.unwrap();
    let result = client
        .send_request(get_account_funds::Parameters { wallet: None })
        .await;

    // Assert: the error code from inside the fault envelope must survive.
    match result {
        Err(ApiError::AccountApingException(exception)) => {
            assert_eq!(
                exception.error_code,
                Some(ErrorCode::InvalidSessionInformation)
            );
        }
        other => panic!("expected AccountApingException, got {other:?}"),
    }
}
