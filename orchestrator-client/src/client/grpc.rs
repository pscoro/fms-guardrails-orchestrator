use http_serde::http::{Extensions, HeaderMap};
use opentelemetry::global;
use opentelemetry_http::HeaderExtractor;
use tonic::metadata::MetadataMap;
use tower::util::BoxCloneService;
use crate::{client::Error, utils::trace};
use crate::utils::trace::with_traceparent_header;

pub type Body = tonic::body::BoxBody;
pub type Request = tonic::Request<Body>;
pub type Response = tonic::Response<Body>;

#[derive(Debug)]
pub struct GrpcClientService(BoxCloneService<Request, Response, Error>);

/// Extracts the `traceparent` header from a gRPC response's metadata and uses it to set the current
/// tracing span context (i.e. use `traceparent` as parent to the current span).
/// Defaults to using the current context when no `traceparent` is found.
/// See https://www.w3.org/TR/trace-context/#trace-context-http-headers-format.
pub fn trace_context_from_grpc_response<T>(response: &tonic::Response<T>) {
    let ctx = global::get_text_map_propagator(|propagator| {
        let metadata = response.metadata().clone();
        // Returns the current context if no `traceparent` is found
        propagator.extract(&HeaderExtractor(&metadata.into_headers()))
    });
    trace::set_span_context(ctx)
}

/// Turns a gRPC client request body of type `T` and header map into a `tonic::Request<T>`.
/// Will also inject the current `traceparent` header into the request based on the current span.
pub fn grpc_request_with_headers<T>(request: T, headers: HeaderMap) -> tonic::Request<T> {
    let headers = with_traceparent_header(headers);
    let metadata = MetadataMap::from_headers(headers);
    tonic::Request::from_parts(metadata, Extensions::new(), request)
}