use crate::error::Error;
use futures_util::stream::Stream;
use std::{
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

pin_project_lite::pin_project! {
    pub(super) struct MapError<E, S> {
        #[pin]
        inner: S,

        _error: PhantomData<E>,
    }
}

pub(super) fn map_crate_error<S>(inner: S) -> MapError<Error, S> {
    map_error(inner)
}

pub(super) fn map_error<S, E>(inner: S) -> MapError<E, S> {
    MapError {
        inner,
        _error: PhantomData,
    }
}

impl<T, StreamErr, E, S> Stream for MapError<E, S>
where
    S: Stream<Item = Result<T, StreamErr>>,
    E: From<StreamErr>,
{
    type Item = Result<T, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        this.inner
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map_err(Into::into)))
    }
}
