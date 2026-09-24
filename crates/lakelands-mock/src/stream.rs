//! The two stream helpers a caller of `ingest` needs, without an async
//! runtime or a stream library: a stream over a list, and the next item
//! of a stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

/// A stream that yields the items of a list, in order, and is always
/// ready.
pub fn of<T: Send + 'static>(items: Vec<T>) -> Pin<Box<dyn Stream<Item = T> + Send>> {
    Box::pin(Items(items.into_iter()))
}

struct Items<T>(std::vec::IntoIter<T>);

impl<T> Unpin for Items<T> {}

impl<T> Stream for Items<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(self.0.next())
    }
}

/// The next item of a stream, or `None` at its end.
pub async fn next<S: Stream + Unpin + ?Sized>(s: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| Pin::new(&mut *s).poll_next(cx)).await
}

/// Every remaining item of a stream.
pub async fn collect<S: Stream + Unpin + ?Sized>(s: &mut S) -> Vec<S::Item> {
    let mut out = Vec::new();
    while let Some(item) = next(s).await {
        out.push(item);
    }
    out
}
