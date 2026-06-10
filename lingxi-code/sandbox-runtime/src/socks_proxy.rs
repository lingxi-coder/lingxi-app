//! Hand-rolled RFC 1928 SOCKS5 (no-auth) allowlist proxy — a behavioral port of
//! `createSocksProxyServer` (`socks-proxy.js`). SECURITY-CRITICAL: it enforces
//! the network allowlist for the opaque TCP tunnels (SSH/git/SOCKS traffic) that
//! the sandbox bridges through the host proxy.
//!
//! The TS uses `@pondwader/socks5-server` with a `setRulesetValidator` +
//! `setConnectionHandler` pair; there is no Rust equivalent and no `socks` crate
//! in the lock, so the protocol is hand-rolled over `tokio`. The wire format is
//! small and well defined, and hand-rolling gives exact control over the
//! pre-connect filter (the security boundary): `is_valid_host` THEN
//! `filter_network_request`, BOTH before any dial.

use crate::host::is_valid_host;

/// SOCKS5 protocol version byte (RFC 1928 §3).
const SOCKS_VERSION: u8 = 0x05;

/// `CONNECT` command (RFC 1928 §4). The sandbox only tunnels CONNECT.
const CMD_CONNECT: u8 = 0x01;

/// `ATYP` = IPv4 address (4 octets, RFC 1928 §5).
const ATYP_IPV4: u8 = 0x01;
/// `ATYP` = fully-qualified domain name (1-byte length prefix + bytes).
const ATYP_DOMAINNAME: u8 = 0x03;
/// `ATYP` = IPv6 address (16 octets).
const ATYP_IPV6: u8 = 0x04;

/// Reply: succeeded / request granted (RFC 1928 §6).
pub const REP_GRANTED: u8 = 0x00;
/// Reply: general SOCKS server failure (used for malformed requests).
pub const REP_GENERAL_FAILURE: u8 = 0x01;
/// Reply: connection not allowed by ruleset (the allowlist rejection).
pub const REP_NOT_ALLOWED: u8 = 0x02;
/// Reply: host unreachable (the dial failed).
pub const REP_HOST_UNREACHABLE: u8 = 0x04;

/// Parse a SOCKS5 CONNECT request body into `(host, port)`.
///
/// Layout (RFC 1928 §4): `VER CMD RSV ATYP DST.ADDR DST.PORT`. Returns `None`
/// unless `VER == 5` and `CMD == CONNECT`, the `ATYP` is one of IPv4 / DOMAINNAME
/// / IPv6, and the buffer holds exactly the address plus the 2-byte big-endian
/// port. DOMAINNAME is decoded with `from_utf8_lossy` — it is an unvalidated
/// length-prefixed byte string, so the caller MUST run [`is_valid_host`] on the
/// result before trusting it (CRLF / null injection defense).
#[must_use]
pub fn parse_request_target(buf: &[u8]) -> Option<(String, u16)> {
    // VER CMD RSV ATYP = 4 header bytes minimum.
    if buf.len() < 4 || buf[0] != SOCKS_VERSION || buf[1] != CMD_CONNECT {
        return None;
    }
    let atyp = buf[3];
    let rest = &buf[4..];
    let (host, port_off) = match atyp {
        ATYP_IPV4 => {
            if rest.len() < 4 {
                return None;
            }
            let host = std::net::Ipv4Addr::new(rest[0], rest[1], rest[2], rest[3]).to_string();
            (host, 4)
        }
        ATYP_DOMAINNAME => {
            let len = *rest.first()? as usize;
            let name = rest.get(1..=len)?;
            (String::from_utf8_lossy(name).into_owned(), 1 + len)
        }
        ATYP_IPV6 => {
            let octets: [u8; 16] = rest.get(0..16)?.try_into().ok()?;
            (std::net::Ipv6Addr::from(octets).to_string(), 16)
        }
        _ => return None,
    };
    // Exactly 2 trailing bytes (big-endian port) must remain.
    let port_bytes = rest.get(port_off..port_off + 2)?;
    if rest.len() != port_off + 2 {
        return None;
    }
    let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
    Some((host, port))
}

/// Test-only reference to `is_valid_host` keeps the import live until Task 2
/// wires the server (which validates every parsed host before dialing).
#[allow(dead_code)]
const _: fn(&str) -> bool = is_valid_host;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_connect_request_ipv4_domain_ipv6() {
        // VER=5 CMD=1 RSV=0 ATYP DST.ADDR DST.PORT
        // IPv4 1.2.3.4:443
        let v4 = [5, 1, 0, 1, 1, 2, 3, 4, 0x01, 0xBB];
        assert_eq!(
            parse_request_target(&v4).unwrap(),
            ("1.2.3.4".to_string(), 443)
        );
        // DOMAINNAME "example.com":80
        let mut d = vec![5, 1, 0, 3, 11];
        d.extend_from_slice(b"example.com");
        d.extend_from_slice(&[0, 80]);
        assert_eq!(
            parse_request_target(&d).unwrap(),
            ("example.com".to_string(), 80)
        );
        // IPv6 ::1:8443
        let mut v6 = vec![5, 1, 0, 4];
        v6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        v6.extend_from_slice(&[0x20, 0xFB]);
        assert_eq!(
            parse_request_target(&v6).unwrap(),
            ("::1".to_string(), 8443)
        );
        // bad CMD (not CONNECT) / bad VER → None
        assert!(parse_request_target(&[5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).is_none());
        assert!(parse_request_target(&[4, 1, 0, 1, 1, 2, 3, 4, 0, 80]).is_none());
    }
}
