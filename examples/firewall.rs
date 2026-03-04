use modulix_core_utils::{firewall, mx};

fn main() {
    firewall::add_global_allow_port(8080, mx::NetworkProtocol::Tcp).unwrap();
    firewall::add_interface_allow_port_range(8080..8090, mx::NetworkProtocol::Udp, "eth0").unwrap();
}
