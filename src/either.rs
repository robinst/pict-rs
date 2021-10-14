use futures_util::stream::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pub(crate) enum Either<Left, Right> {
    Left(Left),
    Right(Right),
}

impl<Left, Right> AsyncRead for Either<Left, Right>
where
    Left: AsyncRead + Unpin,
    Right: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match *self {
            Self::Left(ref mut left) => Pin::new(left).poll_read(cx, buf),
            Self::Right(ref mut right) => Pin::new(right).poll_read(cx, buf),
        }
    }
}

impl<Left, Right> Stream for Either<Left, Right>
where
    Left: Stream<Item = <Right as Stream>::Item> + Unpin,
    Right: Stream + Unpin,
{
    type Item = <Left as Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match *self {
            Self::Left(ref mut left) => Pin::new(left).poll_next(cx),
            Self::Right(ref mut right) => Pin::new(right).poll_next(cx),
        }
    }
}
