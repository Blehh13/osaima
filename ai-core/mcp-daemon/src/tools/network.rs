//! Network diagnostics: interfaces, routes, DNS, and reachability.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::{no_args, read_only, to_value, tool, Args, ToolContext, ToolError, ToolResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const DEFAULT_HOST: &str = "example.com";

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "network_status",
            "Network status",
            "Network interfaces with state, MAC and IP addresses, traffic counters, the default gateway and DNS servers.",
            no_args(),
            read_only(),
        ),
        tool(
            "check_connectivity",
            "Check connectivity",
            "Resolve a host name and open a TCP connection to it, reporting DNS results and connection latency. Use it to diagnose \"no internet\" problems.",
            json!({
                "type": "object",
                "properties": {
                    "host": { "type": "string", "default": DEFAULT_HOST, "description": "Host name or IP address" },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 443 }
                },
                "additionalProperties": false
            }),
            json!({ "readOnlyHint": true, "openWorldHint": true }),
        ),
    ]
}

#[derive(Debug, Serialize)]
struct Interface {
    name: String,
    state: String,
    mac: Option<String>,
    addresses: Vec<String>,
    rx_bytes: Option<u64>,
    tx_bytes: Option<u64>,
}

pub fn network_status(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    let paths = &ctx.paths;
    let addresses = interface_addresses()
        .map_err(|e| ToolError::failed(format!("could not list interfaces: {e}")))?;
    let interfaces: Vec<Interface> = addresses
        .into_iter()
        .map(|(name, addresses)| {
            let dir = paths.net_class_dir.join(&name);
            Interface {
                state: read_trimmed(&dir.join("operstate")).unwrap_or_else(|| "unknown".into()),
                mac: read_trimmed(&dir.join("address")).filter(|m| m != "00:00:00:00:00:00"),
                rx_bytes: read_trimmed(&dir.join("statistics/rx_bytes"))
                    .and_then(|v| v.parse().ok()),
                tx_bytes: read_trimmed(&dir.join("statistics/tx_bytes"))
                    .and_then(|v| v.parse().ok()),
                addresses,
                name,
            }
        })
        .collect();
    let gateway = fs::read_to_string(&paths.proc_net_route)
        .ok()
        .and_then(|text| default_route(&text));
    let dns = fs::read_to_string(&paths.resolv_conf)
        .map(|text| nameservers(&text))
        .unwrap_or_default();
    Ok(json!({
        "interfaces": to_value(&interfaces),
        "default_route": gateway.map(|(iface, gw)| json!({ "interface": iface, "gateway": gw.to_string() })),
        "dns_servers": dns,
    }))
}

pub async fn check_connectivity(arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["host", "port"])?;
    let host = args.str("host")?.map(str::trim).unwrap_or(DEFAULT_HOST);
    validate_host(host)?;
    let port = u16::try_from(args.u64_in("port", 1, 65_535, 443)?)
        .map_err(|_| ToolError::invalid("port must be between 1 and 65535"))?;

    let started = Instant::now();
    let resolved: Vec<SocketAddr> =
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(addrs)) => addrs.collect(),
            Ok(Err(err)) => {
                return Ok(json!({
                    "host": host, "port": port, "dns_ok": false, "reachable": false,
                    "error": format!("DNS lookup failed: {err}")
                }))
            }
            Err(_) => {
                return Ok(json!({
                    "host": host, "port": port, "dns_ok": false, "reachable": false,
                    "error": "DNS lookup timed out"
                }))
            }
        };
    let dns_ms = started.elapsed().as_millis() as u64;
    let addresses: Vec<String> = resolved.iter().map(|a| a.ip().to_string()).collect();

    let Some(target) = resolved.first().copied() else {
        return Ok(json!({
            "host": host, "port": port, "dns_ok": false, "reachable": false,
            "error": "the name resolved to no addresses"
        }));
    };
    let connect_started = Instant::now();
    let (reachable, error) =
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(target)).await {
            Ok(Ok(_)) => (true, None),
            Ok(Err(err)) => (false, Some(format!("connection failed: {err}"))),
            Err(_) => (false, Some("connection timed out".to_string())),
        };
    Ok(json!({
        "host": host,
        "port": port,
        "dns_ok": true,
        "dns_ms": dns_ms,
        "addresses": addresses,
        "reachable": reachable,
        "connect_ms": reachable.then(|| connect_started.elapsed().as_millis() as u64),
        "error": error,
    }))
}

/// Host names and IP literals only: letters, digits, `.`, `-`, `:`.
fn validate_host(host: &str) -> Result<(), ToolError> {
    let ok = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    if ok || host.parse::<IpAddr>().is_ok() {
        Ok(())
    } else {
        Err(ToolError::invalid(format!(
            "{host:?} is not a valid host name"
        )))
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// The default IPv4 route from `/proc/net/route`: (interface, gateway).
fn default_route(table: &str) -> Option<(String, Ipv4Addr)> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[1] != "00000000" {
            return None;
        }
        // The kernel prints the address as a host-endian (little-endian) hex word.
        let raw = u32::from_str_radix(fields[2], 16).ok()?;
        Some((fields[0].to_string(), Ipv4Addr::from(raw.to_le_bytes())))
    })
}

fn nameservers(resolv_conf: &str) -> Vec<String> {
    resolv_conf
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Interface name → IP addresses, from getifaddrs(3). Interfaces without an
/// address are included with an empty list.
fn interface_addresses() -> io::Result<BTreeMap<String, Vec<String>>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: on success getifaddrs stores a list we release with freeifaddrs below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the list getifaddrs returned and is not freed yet.
        let entry = unsafe { &*cursor };
        // SAFETY: ifa_name is a valid NUL-terminated string for the node's lifetime.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let addresses = map.entry(name).or_default();
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_addr is non-null; the family field tells us the concrete type.
            let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
            if family == libc::AF_INET {
                // SAFETY: AF_INET addresses are sockaddr_in.
                let sin = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                addresses.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)).to_string());
            } else if family == libc::AF_INET6 {
                // SAFETY: AF_INET6 addresses are sockaddr_in6.
                let sin6 = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                addresses.push(Ipv6Addr::from(sin6.sin6_addr.s6_addr).to_string());
            }
        }
        cursor = entry.ifa_next;
    }
    // SAFETY: `head` came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    const ROUTE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
        eth0\t0010A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n\
        eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\n";

    #[test]
    fn parses_default_route() {
        assert_eq!(
            default_route(ROUTE),
            Some(("eth0".into(), Ipv4Addr::new(192, 168, 1, 1)))
        );
        assert_eq!(default_route("Iface\tDestination\n"), None);
    }

    #[test]
    fn parses_nameservers() {
        let conf = "# comment\nnameserver 1.1.1.1\nsearch lan\nnameserver ::1 \n";
        assert_eq!(nameservers(conf), ["1.1.1.1", "::1"]);
    }

    #[test]
    fn validates_hosts() {
        for ok in ["example.com", "192.168.1.1", "::1", "gentoo.org"] {
            assert!(validate_host(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-oProxyCommand=x", "a b", "evil.com;rm", "$(id)"] {
            assert!(validate_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_lists_loopback_with_files_from_host_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        std::fs::create_dir_all(dir.path().join("net/lo/statistics")).unwrap();
        std::fs::write(dir.path().join("net/lo/operstate"), "unknown\n").unwrap();
        std::fs::write(dir.path().join("net/lo/statistics/rx_bytes"), "42\n").unwrap();
        std::fs::write(dir.path().join("route"), ROUTE).unwrap();
        std::fs::write(dir.path().join("resolv.conf"), "nameserver 9.9.9.9\n").unwrap();

        let status = network_status(&ctx, None).unwrap();
        let lo = status["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "lo")
            .expect("loopback interface");
        assert_eq!(lo["rx_bytes"], 42);
        assert!(lo["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "127.0.0.1"));
        assert_eq!(status["default_route"]["gateway"], "192.168.1.1");
        assert_eq!(status["dns_servers"], json!(["9.9.9.9"]));
    }

    #[tokio::test]
    async fn connectivity_to_a_local_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let res = check_connectivity(Some(json!({ "host": "127.0.0.1", "port": port })))
            .await
            .unwrap();
        assert_eq!(res["reachable"], true);
        assert_eq!(res["dns_ok"], true);
    }

    #[tokio::test]
    async fn connectivity_reports_refused_connections() {
        // Bind then drop to get a port with nothing listening.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let res = check_connectivity(Some(json!({ "host": "127.0.0.1", "port": port })))
            .await
            .unwrap();
        assert_eq!(res["reachable"], false);
        assert!(res["error"].as_str().unwrap().contains("connection"));
    }
}
