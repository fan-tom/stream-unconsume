# stream-unconsume

Poll stream partially and get emitted items back!
Sometimes it is useful to let someone poll you stream but be able to get it back as if it was never polled.
To do so, this crate provides function `remember`, that, given stream, returns tuple of future and stream.
You can move returned stream to any function, losing it, but, when it gets dropped,
future, returned from `remember`, is resolved with new stream that provides all items that was consumed from
original stream, as well as the rest of this stream, so you can use it as if you never poll original stream.

You may specify any type to be used as buffer, as long as it implements `IntoIterator<Item=Source::Item>` and
`PushBack<Source::Item>`, where `Source` is the type of stream you want to unconsume.
There is convenience function `remember_vec`, to use `Vec` as buffer backend
Note: Rust doesn't guarantee that Drop is ever called, so you may need to use timeout when you await returned future,
otherwise you will wait for it's resolve forever!

Current Version: 0.3.0

License: MIT OR Apache-2.0
