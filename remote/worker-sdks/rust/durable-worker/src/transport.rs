use crate::error::TransportError;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Method, Url};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

pub type TransportFuture<'a> = Pin<
    Box<dyn Future<Output = Result<TransportResponse, TransportError>> + Send + 'a>,
>;

#[derive(Clone, Debug)]
pub struct TransportRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    fn execute<'a>(&'a self, request: TransportRequest) -> TransportFuture<'a>;
}

#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| TransportError::new(error.to_string(), false))?;
        Ok(Self { client })
    }
}

impl Transport for ReqwestTransport {
    fn execute<'a>(&'a self, request: TransportRequest) -> TransportFuture<'a> {
        Box::pin(async move {
            let method = Method::from_bytes(request.method.as_bytes())
                .map_err(|error| TransportError::new(error.to_string(), false))?;
            let url = Url::parse(&request.url)
                .map_err(|error| TransportError::new(error.to_string(), false))?;
            let mut builder = self.client.request(method, url).timeout(request.timeout);
            for (name, value) in request.headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| TransportError::new(error.to_string(), false))?;
                let value = HeaderValue::from_str(&value)
                    .map_err(|error| TransportError::new(error.to_string(), false))?;
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }

            let mut response = builder.send().await.map_err(|error| {
                TransportError::new(
                    error.to_string(),
                    error.is_timeout() || error.is_connect() || error.is_body(),
                )
            })?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
                })
                .collect::<BTreeMap<_, _>>();

            if response
                .content_length()
                .is_some_and(|length| length > request.max_response_bytes as u64)
            {
                return Err(TransportError::new(
                    format!(
                        "response exceeded {} bytes",
                        request.max_response_bytes
                    ),
                    false,
                ));
            }

            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| TransportError::new(error.to_string(), true))?
            {
                if body.len().saturating_add(chunk.len()) > request.max_response_bytes {
                    return Err(TransportError::new(
                        format!(
                            "response exceeded {} bytes",
                            request.max_response_bytes
                        ),
                        false,
                    ));
                }
                body.extend_from_slice(&chunk);
            }

            Ok(TransportResponse {
                status,
                headers,
                body,
            })
        })
    }
}
