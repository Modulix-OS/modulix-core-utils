use modulix_core_utils::{CONFIG_DIRECTORY, firewall, mx};

fn main() {
    firewall::add_global_allow_port(CONFIG_DIRECTORY, 8080, mx::NetworkProtocol::Tcp).unwrap();
    firewall::add_interface_allow_port_range(
        CONFIG_DIRECTORY,
        8080..8090,
        mx::NetworkProtocol::Udp,
        "eth0",
    )
    .unwrap();
}
