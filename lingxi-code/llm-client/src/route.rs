use crate::{ResolvedRoute, WireCodec};

#[derive(Debug)]
pub struct Route {
    pub resolved_route: ResolvedRoute,
    pub codec: Box<dyn WireCodec>,
}
