use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Middleware that ensures every request has an X-Request-Id header.
pub async fn ensure_request_id(mut request: Request, next: Next) -> Response {
    if !request.headers().contains_key(&X_REQUEST_ID) {
        let request_id = Uuid::new_v4().to_string();
        request.headers_mut().insert(
            X_REQUEST_ID.clone(),
            HeaderValue::from_str(&request_id).unwrap(),
        );
    }

    let mut response = next.run(request).await;

    // Propagate request ID to response
    if let Some(value) = response.headers().get(&X_REQUEST_ID).cloned() {
        response.headers_mut().insert(X_REQUEST_ID.clone(), value);
    }

    response
}
