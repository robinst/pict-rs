use tracing::{Id, Span};

pub(crate) enum IdOrSpan {
    Id(Option<Id>),
    Span(Span),
}

impl IdOrSpan {
    pub(crate) fn take(&mut self) -> Self {
        std::mem::replace(self, IdOrSpan::Id(None))
    }

    pub(crate) fn from_id(id: Option<Id>) -> Self {
        IdOrSpan::Id(id)
    }

    fn span(&self) -> Option<&Span> {
        match self {
            IdOrSpan::Span(ref span) => Some(span),
            _ => None,
        }
    }

    pub(crate) fn as_span(&mut self, f: impl Fn(Option<Id>) -> Span) -> &Span {
        let span = match self.take() {
            Self::Id(opt) => f(opt),
            Self::Span(span) => span,
        };

        *self = Self::Span(span);

        self.span().expect("Span should always exist")
    }
}
