use crate::{ProtocolFamily, ResolvedRoute, WireCodec};

#[derive(Debug)]
pub struct Route {
    pub resolved_route: ResolvedRoute,
    pub protocol: ProtocolFamily,
    pub codec: Box<dyn WireCodec>,
}
