use reqwest_middleware::ClientWithMiddleware;

use crate::{config::Configuration, magick::ArcPolicyDir, repo::ArcRepo, tmp_file::ArcTmpDir};

#[derive(Clone)]
pub(crate) struct State<S> {
    pub(super) config: Configuration,
    pub(super) tmp_dir: ArcTmpDir,
    pub(super) policy_dir: ArcPolicyDir,
    pub(super) repo: ArcRepo,
    pub(super) store: S,
    pub(super) client: ClientWithMiddleware,
}
