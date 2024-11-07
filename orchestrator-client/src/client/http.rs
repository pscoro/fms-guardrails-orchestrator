use std::pin::Pin;
use hyper::body::Bytes;
use opentelemetry::global;
use opentelemetry_http::HeaderExtractor;
use tower::util::BoxCloneService;
use crate::{client::Error, utils::trace};

pub type Body = Pin<Box<dyn hyper::body::Body<Data=Bytes, Error=Error> + Send>>;
pub type Request = hyper::Request<Body>;
pub type Response = hyper::Response<Body>;

#[derive(Debug)]
pub struct HttpClientService(BoxCloneService<Request, Response, Error>);

/// Extracts the `traceparent` header from an HTTP response's headers and uses it to set the current
/// tracing span context (i.e. use `traceparent` as parent to the current span).
/// Defaults to using the current context when no `traceparent` is found.
/// See https://www.w3.org/TR/trace-context/#trace-context-http-headers-format.
pub fn trace_context_from_response(response: &Response) {
    let ctx = global::get_text_map_propagator(|propagator| {
        // Returns the current context if no `traceparent` is found
        propagator.extract(&HeaderExtractor(response.headers()))
    });
    trace::set_span_context(ctx);
}