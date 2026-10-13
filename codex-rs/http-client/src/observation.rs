//! Read-only, task-scoped observations of native transport activity.
//!
//! These callbacks cannot change requests, retry decisions or response consumption.
//! Background response readers must capture the observer before spawning their task.

use http::HeaderMap;
use std::future::Future;
use std::sync::Arc;

pub trait TransportObserver: Send + Sync {
    /// Preserve the original error response before typed API classification.
    fn http_error(&self, _status: u16, _headers: &HeaderMap, _body: Option<&str>) {}
    fn observes_request(&self, _method: &str, _url: &str) -> bool {
        true
    }
    fn request(&self, transport: &str, method: &str, url: &str, headers: &HeaderMap, body: &[u8]);
    fn response(&self, transport: &str, status: u16, headers: &HeaderMap);
    fn body(&self, transport: &str, bytes: &[u8]);
    fn error(&self, transport: &str, message: &str);
}

tokio::task_local! {
    static OBSERVER: Arc<dyn TransportObserver>;
}

pub async fn observe_transport<F: Future>(
    observer: Arc<dyn TransportObserver>,
    future: F,
) -> F::Output {
    OBSERVER.scope(observer, future).await
}

pub fn transport_observer() -> Option<Arc<dyn TransportObserver>> {
    OBSERVER.try_with(Arc::clone).ok()
}
