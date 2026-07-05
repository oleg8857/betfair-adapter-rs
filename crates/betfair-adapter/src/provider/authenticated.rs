use core::fmt;
use core::marker::PhantomData;

use betfair_types::keep_alive;
use betfair_types::types::BetfairRpcRequest;
use tracing::instrument;

use crate::{ApiError, Authenticated, BetfairRpcClient};

impl BetfairRpcClient<Authenticated> {
    /// Sends a request and returns the response or an error.
    ///
    /// # Parameters
    /// - `request`: The request to be sent.
    ///
    /// # Returns
    /// A result containing either the response or an `ApiError`.
    #[tracing::instrument(skip_all, err, fields(req = ?request))]
    pub async fn send_request<T>(&self, request: T) -> Result<T::Res, ApiError>
    where
        T: BetfairRpcRequest + serde::Serialize + core::fmt::Debug,
        T::Res: serde::de::DeserializeOwned + core::fmt::Debug,
        T::Error: serde::de::DeserializeOwned,
        ApiError: From<<T as BetfairRpcRequest>::Error>,
    {
        let endpoint = self.rest_base.url().join(T::method())?;
        let full = self
            .state
            .authenticated_client
            .post(endpoint.as_str())
            .json(&request)
            .send()
            .await?;

        if full.status().is_success() {
            let text = full.text().await?;
            if text.trim().is_empty() {
                tracing::warn!("Received empty response body");
                return Err(ApiError::EmptyResponse);
            }
            let res = serde_json::from_str::<T::Res>(&text)?;
            Ok(res)
        } else {
            let status = full.status();
            let bytes = full.bytes().await?;
            let res = parse_betfair_error::<T::Error>(&bytes, status)?;
            Err(res.into())
        }
    }

    /// Create a request
    ///
    /// # Parameters
    /// - `request`: The request to be sent.
    ///
    /// # Returns
    /// A result containing either the response or an `ApiError`.
    pub fn build_request<T>(&self, request: T) -> Result<BetfairRequest<T::Res, T::Error>, ApiError>
    where
        T: BetfairRpcRequest + serde::Serialize + core::fmt::Debug,
        T::Res: serde::de::DeserializeOwned + core::fmt::Debug,
        T::Error: serde::de::DeserializeOwned,
    {
        let endpoint = self.rest_base.url().join(T::method())?;
        let client = self.state.authenticated_client.clone();
        let reqwest_req = client
            .request(reqwest::Method::POST, endpoint.as_str())
            .json(&request)
            .build()?;

        Ok(BetfairRequest {
            request: reqwest_req,
            client,
            result: PhantomData,
            err: PhantomData,
        })
    }

    /// You can use Keep Alive to extend the session timeout period. The minimum session time is
    /// currently 20 minutes (Italian Exchange). On the international (.com) Exchange the current
    /// session time is 24 hours. Therefore, you should request Keep Alive within this time to
    /// prevent session expiry. If you don't call Keep Alive within the specified timeout period,
    /// the session will expire. Session times aren't determined or extended based on API activity.
    #[tracing::instrument(skip_all, err)]
    pub fn keep_alive(&self) -> Result<BetfairRequest<keep_alive::Response, ()>, ApiError> {
        let endpoint = self.keep_alive.url();
        let client = self.state.authenticated_client.clone();
        let reqwest_req = client
            .request(reqwest::Method::GET, endpoint.as_str())
            .build()?;

        Ok(BetfairRequest {
            request: reqwest_req,
            client,
            result: PhantomData,
            err: PhantomData,
        })
    }

    /// You can use Logout to terminate your existing session.
    #[tracing::instrument(skip_all, err)]
    pub fn logout(&self) -> Result<BetfairRequest<keep_alive::Response, ()>, ApiError> {
        let endpoint = self.logout.url();
        let client = self.state.authenticated_client.clone();
        let reqwest_req = client
            .request(reqwest::Method::GET, endpoint.as_str())
            .build()?;

        Ok(BetfairRequest {
            request: reqwest_req,
            client,
            result: PhantomData,
            err: PhantomData,
        })
    }
}

/// Encalpsulated HTTP request for the Betfair API
pub struct BetfairRequest<T, E> {
    request: reqwest::Request,
    client: reqwest::Client,
    result: PhantomData<T>,
    err: PhantomData<E>,
}

/// HTTP header names whose values must never be written to logs.
///
/// `reqwest`/`http` normalise header names to lowercase, so comparing against
/// these lowercase literals is effectively case-insensitive.
const SENSITIVE_HEADERS: &[&str] = &["x-authentication", "authorization", "cookie", "set-cookie"];

// Manual `Debug` impl: the derived one prints the `client`, whose default
// headers carry the `x-authentication` session token, leaking it into logs.
// We omit the `client` entirely and redact sensitive headers of the request.
impl<T, E> fmt::Debug for BetfairRequest<T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BetfairRequest")
            .field("method", self.request.method())
            .field("url", &self.request.url().as_str())
            .field("headers", &RedactedHeaders(self.request.headers()))
            .finish_non_exhaustive()
    }
}

/// Wraps a `HeaderMap` to redact the values of sensitive headers when formatted
/// with `Debug`.
struct RedactedHeaders<'a>(&'a reqwest::header::HeaderMap);

impl fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = formatter.debug_map();
        for (name, value) in self.0 {
            if SENSITIVE_HEADERS.contains(&name.as_str()) {
                map.entry(&name.as_str(), &"<redacted>");
            } else {
                map.entry(&name.as_str(), &value);
            }
        }
        map.finish()
    }
}

impl<T, E> BetfairRequest<T, E> {
    /// execute an Betfair API request
    #[instrument(name = "execute_request", skip(self), fields(method = %self.request.method(), url = %self.request.url()))]
    pub async fn execute(self) -> Result<BetfairResponse<T, E>, ApiError> {
        let response = self.client.execute(self.request).await?;

        // Capture the current span
        let span = tracing::Span::current();

        Ok(BetfairResponse {
            response,
            result: PhantomData,
            err: PhantomData,
            span,
        })
    }
}

/// The raw response of the Betfair API request
#[derive(Debug)]
pub struct BetfairResponse<T, E> {
    response: reqwest::Response,
    result: PhantomData<T>,
    err: PhantomData<E>,
    // this span carries the context of the `BetfairRequest`
    span: tracing::Span,
}

impl<T, E> BetfairResponse<T, E> {
    /// Only check if the returtned HTTP response is of error type; don't parse the data
    ///
    /// Useful when you don't care about the actual response besides if it was an error.
    #[instrument(name = "response_ok", skip(self), err, parent = &self.span)]
    pub fn ok(self) -> Result<(), ApiError> {
        self.response.error_for_status()?;
        Ok(())
    }

    /// Check if the returned HTTP result is an error;
    /// Only parse the error type if we received an error.
    ///
    /// Useful when you don't care about the actual response besides if it was an error.
    #[instrument(name = "parse_response_json_err", skip(self), err, parent = &self.span)]
    pub async fn json_err(self) -> Result<Result<(), E>, ApiError>
    where
        E: serde::de::DeserializeOwned,
    {
        let status = self.response.status();
        if status.is_success() {
            Ok(Ok(()))
        } else {
            let bytes = self.response.bytes().await?;
            let res = parse_betfair_error::<E>(&bytes, status)?;
            Ok(Err(res))
        }
    }

    /// Parse the response json
    #[instrument(name = "parse_response_json", skip(self), err, parent = &self.span)]
    pub async fn json(self) -> Result<Result<T, E>, ApiError>
    where
        T: serde::de::DeserializeOwned,
        E: serde::de::DeserializeOwned,
    {
        let status = self.response.status();
        let bytes = self.response.bytes().await?;
        if status.is_success() {
            let json = String::from_utf8_lossy(bytes.as_ref());
            tracing::debug!(response_body = %json, "Response JSON");

            let res = serde_json::from_slice::<T>(&bytes)?;
            Ok(Ok(res))
        } else {
            let res = parse_betfair_error::<E>(&bytes, status)?;
            Ok(Err(res))
        }
    }
}

fn parse_betfair_error<E>(bytes: &[u8], status: reqwest::StatusCode) -> Result<E, ApiError>
where
    E: serde::de::DeserializeOwned,
{
    let json = String::from_utf8_lossy(bytes);
    tracing::error!(
        status = %status,
        body = %json,
        "Failed to execute request"
    );

    // Betfair REST exceptions are wrapped in a SOAP-like fault envelope:
    // `{faultcode, faultstring, detail: {exceptionname: "X", X: {..real fields..}}}`.
    // The generated exception types (`E`) are flat, so the actual `errorCode`
    // lives at `detail.<exceptionname>`, not at the root. Unwrap it here before
    // deserializing `E`; fall back to the whole body for non-enveloped payloads.
    if let Ok(envelope) = serde_json::from_slice::<serde_json::Value>(bytes)
        && let Some(name) = envelope
            .get("detail")
            .and_then(|detail| detail.get("exceptionname"))
            .and_then(serde_json::Value::as_str)
        && let Some(inner) = envelope.get("detail").and_then(|detail| detail.get(name))
    {
        return Ok(serde_json::from_value::<E>(inner.clone())?);
    }

    // Fallback: body without the fault envelope.
    let error = serde_json::from_slice::<E>(bytes)?;
    Ok(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "super-secret-session-token";

    /// Builds a `BetfairRequest` whose client *and* request both carry the
    /// session token, mirroring how `logged_in_client` sets the default header.
    fn request_carrying_token() -> BetfairRequest<(), ()> {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            "X-Authentication",
            reqwest::header::HeaderValue::from_static(TOKEN),
        );
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .default_headers(default_headers)
            .build()
            .expect("client builds");

        let request = client
            .get("https://api.betfair.com/keepAlive")
            .header("X-Authentication", TOKEN)
            .build()
            .expect("request builds");

        BetfairRequest {
            request,
            client,
            result: PhantomData,
            err: PhantomData,
        }
    }

    #[test]
    fn debug_does_not_leak_session_token() {
        let formatted = format!("{:?}", request_carrying_token());

        assert!(
            !formatted.contains(TOKEN),
            "session token leaked through Debug:\n{formatted}"
        );
        assert!(
            formatted.contains("<redacted>"),
            "expected redacted header marker in Debug output:\n{formatted}"
        );
    }
}
