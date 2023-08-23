use futures_core::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pin_project_lite::pin_project! {
    #[project = EitherProj]
    #[project_replace = EitherProjReplace]
    pub(crate) enum Either<Left, Right> {
        Left {
            #[pin]
            left: Left,
        },
        Right {
            #[pin]
            right: Right,
        },
    }
}

impl<L, R> Either<L, R> {
    pub(crate) fn left(left: L) -> Self {
        Either::Left { left }
    }

    pub(crate) fn right(right: R) -> Self {
        Either::Right { right }
    }
}

impl<Left, Right> AsyncRead for Either<Left, Right>
where
    Left: AsyncRead,
    Right: AsyncRead,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        match this {
            EitherProj::Left { left } => left.poll_read(cx, buf),
            EitherProj::Right { right } => right.poll_read(cx, buf),
        }
    }
}

impl<Left, Right> Stream for Either<Left, Right>
where
    Left: Stream<Item = <Right as Stream>::Item>,
    Right: Stream,
{
    type Item = <Left as Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        match this {
            EitherProj::Left { left } => left.poll_next(cx),
            EitherProj::Right { right } => right.poll_next(cx),
        }
    }
}
