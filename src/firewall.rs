use std::ops::Range;

use crate::core::list::List as mxList;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::BuildCommand;
use crate::{core::transaction, mx};

pub enum NetworkProtocol {
    Udp,
    Tcp,
}

const FILE_FIREWALL_PATH: &str = "firewall.nix";

impl NetworkProtocol {
    pub fn as_str(&self) -> &str {
        match self {
            NetworkProtocol::Tcp => "TCP",
            NetworkProtocol::Udp => "UDP",
        }
    }
}

pub fn add_global_allow_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
    mxList::new(&option_name, true).add(file, &allowed_port.to_string())?;
    Ok(())
}

pub fn remove_global_allowed_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
    mxList::new(&option_name, true).remove(file, &allowed_port.to_string())?;
    Ok(())
}

pub fn add_global_allowed_port_range_no_transaction(
    file: &mut NixFile,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}PortRanges", protocol.as_str());
    mxList::new(&option_name, true).add(
        file,
        &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
    )?;
    Ok(())
}

pub fn remove_global_allowed_port_range_no_transaction(
    file: &mut NixFile,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}PortRanges", protocol.as_str());
    mxList::new(&option_name, true).remove(
        file,
        &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
    )?;
    Ok(())
}

pub fn add_interface_allow_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    let option_name = format!(
        "networking.firewall.interfaces.\"{}\".allowed{}Ports",
        interface,
        protocol.as_str()
    );
    mxList::new(&option_name, true).add(file, &allowed_port.to_string())?;
    Ok(())
}

pub fn remove_interface_allowed_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    let option_name = format!(
        "networking.firewall.interfaces.\"{}\".allowed{}Ports",
        interface,
        protocol.as_str()
    );
    mxList::new(&option_name, true).remove(file, &allowed_port.to_string())?;
    Ok(())
}

pub fn add_interface_allow_port_range_no_transaction(
    file: &mut NixFile,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    let option_name = format!(
        "networking.firewall.interfaces.\"{}\".allowed{}PortRanges",
        interface,
        protocol.as_str()
    );
    mxList::new(&option_name, true).add(
        file,
        &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
    )?;
    Ok(())
}

pub fn remove_interface_allowed_port_range_no_transaction(
    file: &mut NixFile,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    let option_name = format!(
        "networking.firewall.interfaces.\"{}\".allowed{}PortRanges",
        interface,
        protocol.as_str()
    );
    mxList::new(&option_name, true).remove(
        file,
        &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
    )?;
    Ok(())
}

pub fn add_global_allow_port(
    config_dir: &str,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Allow {} {} port", allowed_port, protocol.as_str()),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| add_global_allow_port_no_transaction(file, allowed_port, protocol),
    )
}

pub fn remove_global_allowed_port(
    config_dir: &str,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove allowed {} {} port", allowed_port, protocol.as_str()),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| remove_global_allowed_port_no_transaction(file, allowed_port, protocol),
    )
}

pub fn add_global_allowed_port_range(
    config_dir: &str,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Allow {} to {} {} ports",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str()
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| add_global_allowed_port_range_no_transaction(file, allowed_ports, protocol),
    )
}

pub fn remove_global_allowed_port_range(
    config_dir: &str,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Remove allowed {} to {} {} ports range",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str()
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| remove_global_allowed_port_range_no_transaction(file, allowed_ports, protocol),
    )
}

pub fn add_interface_allow_port(
    config_dir: &str,
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Allow {} {} port for interface {}",
            allowed_port,
            protocol.as_str(),
            interface
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| add_interface_allow_port_no_transaction(file, allowed_port, protocol, interface),
    )
}

pub fn remove_interface_allowed_port(
    config_dir: &str,
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Remove allowed {} {} port for interface {}",
            allowed_port,
            protocol.as_str(),
            interface
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| {
            remove_interface_allowed_port_no_transaction(file, allowed_port, protocol, interface)
        },
    )
}

pub fn add_interface_allow_port_range(
    config_dir: &str,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Allow {} to {} {} ports for interface {}",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str(),
            interface
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| {
            add_interface_allow_port_range_no_transaction(file, allowed_ports, protocol, interface)
        },
    )
}

pub fn remove_interface_allowed_port_range(
    config_dir: &str,
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!(
            "Remove allowed {} to {} {} ports for interface {}",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str(),
            interface
        ),
        config_dir,
        FILE_FIREWALL_PATH,
        BuildCommand::Switch,
        |file| {
            remove_interface_allowed_port_range_no_transaction(
                file,
                allowed_ports,
                protocol,
                interface,
            )
        },
    )
}
