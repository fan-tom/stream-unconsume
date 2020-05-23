//! Poll stream partially and get emitted items back!

//! Sometimes it is useful to let someone poll you stream but be able to get it back as if it was never polled.
//! To do so, this crate provides function `split`, that, given stream, returns tuple of future and stream.
//! You can move returned stream to any function, losing it, but, when it gets dropped,
//! future, returned from `split`, is resolved with new stream that provides all items that was consumed from
//! original stream, as well as the rest of this stream, so you can use it as if you never poll original stream.

//! Note: Rust doesn't guarantee that Drop is ever called, so you may need to use timeout when you await returned future,
//! otherwise you will wait for it's resolve forever!

use futures::channel::oneshot::Canceled;
use futures::task::{Context, Poll};
use futures::{
    channel::oneshot::{self, Sender},
    stream::iter,
    Future, Stream, StreamExt,
};
use pin_project::{pin_project, pinned_drop};
use std::mem::ManuallyDrop;
use std::ops::DerefMut;
use std::pin::Pin;

// We require Unpin here to be able to move stream out of ManuallyDrop in Drop
#[pin_project(PinnedDrop)]
pub struct StreamBuffer<S: Stream + Unpin> {
    // we need to extract these fields in destructor
    // pin stream as we need to poll it
    #[pin]
    inner: ManuallyDrop<S>,
    buffer: ManuallyDrop<Vec<S::Item>>,
    tx: Option<Sender<(S, Vec<S::Item>)>>,
}

impl<S: Stream + Unpin> StreamBuffer<S> {
    // like `self.project().inner`, but derefs further to underlying stream
    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        // SAFETY: we just derefs further, moving S will be impossible, and we don't move anything in closure itself
        unsafe { Pin::map_unchecked_mut(self, |x| x.inner.deref_mut()) }
    }

    fn new(source: S, tx: Sender<(S, Vec<S::Item>)>) -> Self {
        StreamBuffer {
            inner: ManuallyDrop::new(source),
            buffer: ManuallyDrop::new(Vec::new()),
            tx: Some(tx),
        }
    }
}

impl<S: Stream + Unpin> Stream for StreamBuffer<S>
where
    S::Item: Clone,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = self.as_mut().stream().poll_next(cx);
        if let Poll::Ready(Some(item)) = &next {
            self.project().buffer.push((*item).clone());
        }
        next
    }
}

#[pinned_drop]
impl<S: Stream + Unpin> PinnedDrop for StreamBuffer<S> {
    fn drop(self: Pin<&mut Self>) {
        let mut this = self.project();
        // SAFETY: we take Sender<T> because we never pinned it and no one has references to/into it
        let tx = this.tx.take().expect("Sender is gone");
        // SAFETY: we don't use inner nor buffer after this line, they are not touched by Drop too
        // ignore error as we don't care if receiver no more interested in stream and buffer
        let _ = tx.send((
            // SAFETY: We required S to be Unpin, so here we can move it out of ManuallyDrop
            unsafe { ManuallyDrop::take(Pin::into_inner(this.inner)) },
            // SAFETY: We don't need `S::Item`s to be Unpin because we never pin them,
            // and Vec<S::Item> can be moved out of ManuallyDrop because we never pin it
            unsafe { ManuallyDrop::take(&mut this.buffer) },
        ));
        // we don't call ManuallyDrop on fields as they are moved to channel
    }
}

/// Returns stream that remembers all produced items
/// And resolves returned future with stream that behaves like original stream was never polled
/// In other words, it lets partially consume stream and get all consumed items back
pub fn split<S: Stream + Unpin + 'static>(
    source: S,
) -> (
    impl Future<Output = Result<impl Stream<Item = S::Item> + 'static, Canceled>>,
    impl Stream<Item = S::Item>,
)
where
    S::Item: Clone,
{
    let (tx, rx) = oneshot::channel();
    let fut = async {
        let (tail, buffer) = rx.await?;
        Ok(iter(buffer).chain(tail))
    };
    // fuse source stream to be able to poll it after finish (when we `chain` it with buffer)
    (fut, StreamBuffer::new(source.fuse(), tx))
}

#[cfg(test)]
mod tests {
    use super::split;
    use futures::channel::oneshot::Canceled;
    use futures::executor::block_on;
    use futures::future::ready;
    use futures::StreamExt;

    #[test]
    fn test_consumed_values_are_present() {
        let x = vec![1, 2, 3];
        let source = futures::stream::iter(x.clone().into_iter());
        let (buffer, buffer_stream) = split(source);
        block_on(async {
            // consume first two items
            buffer_stream.take(2).for_each(|_| ready(())).await;
            let stream = buffer.await?;
            // first two items are still present after stream comes back
            assert_eq!(stream.collect::<Vec<_>>().await, x);
            Ok::<_, Canceled>(())
        });
    }

    #[test]
    fn test_consumed_stream_becomes_empty_tail_and_dont_panic() {
        struct UnfusedIter<I> {
            finished: bool,
            inner: I,
        }
        impl<I> UnfusedIter<I> {
            fn new(inner: I) -> UnfusedIter<I> {
                Self {
                    inner,
                    finished: false,
                }
            }
        }

        impl<I: Iterator> Iterator for UnfusedIter<I> {
            type Item = I::Item;

            fn next(&mut self) -> Option<Self::Item> {
                if self.finished {
                    panic!("Iterating over finished iterator");
                } else {
                    let next = self.inner.next();
                    if next.is_none() {
                        self.finished = true;
                    }
                    next
                }
            }
        }
        let x = vec![1, 2, 3];
        // Here we want to emulate stream that panics on poll after finish
        let source = futures::stream::iter(UnfusedIter::new(x.clone().into_iter()));
        let (buffer, buffer_stream) = split(source);
        block_on(async {
            // consume whole stream
            buffer_stream.for_each(|_| ready(())).await;
            let stream = buffer.await?;
            // consumed stream doesn't panic on consume after finish
            assert_eq!(stream.collect::<Vec<_>>().await, x);
            Ok::<_, Canceled>(())
        });
    }
}
