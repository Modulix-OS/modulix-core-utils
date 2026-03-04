use std::ops::Range;

use crate::core::list::List as mxList;
use crate::core::transaction::file_lock::NixFile;
use crate::{
    core::transaction::{Transaction, transaction::BuildCommand},
    mx,
};

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

fn with_firewall_transaction<F>(description: &str, f: F) -> mx::Result<()>
where
    F: FnOnce(&mut NixFile) -> mx::Result<()>,
{
    let mut transaction = Transaction::new(description, BuildCommand::Switch)?;
    transaction.add_file(FILE_FIREWALL_PATH)?;
    transaction.begin()?;

    let file = match transaction.get_file(FILE_FIREWALL_PATH) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match f(file) {
        Ok(()) => transaction.commit(),
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

pub fn add_global_allow_port(allowed_port: u32, protocol: NetworkProtocol) -> mx::Result<()> {
    with_firewall_transaction(
        &format!("Allow {} {} port", allowed_port, protocol.as_str()),
        |file| {
            let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
            mxList::new(&option_name, true).add(file, &allowed_port.to_string())
        },
    )
}

pub fn remove_global_allowed_port(allowed_port: u32, protocol: NetworkProtocol) -> mx::Result<()> {
    with_firewall_transaction(
        &format!("Remove allowed {} {} port", allowed_port, protocol.as_str()),
        |file| {
            let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
            mxList::new(&option_name, true).remove(file, &allowed_port.to_string())
        },
    )
}

pub fn add_global_allowed_port_range(
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Allow {} to {} {} ports",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str()
        ),
        |file| {
            let option_name = format!("networking.firewall.allowed{}PortRanges", protocol.as_str());
            mxList::new(&option_name, true).add(
                file,
                &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
            )
        },
    )
}

pub fn remove_global_allowed_port_range(
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Remove allowed {} to {} {} ports range",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str()
        ),
        |file| {
            let option_name = format!("networking.firewall.allowed{}PortRanges", protocol.as_str());
            mxList::new(&option_name, true).remove(
                file,
                &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
            )
        },
    )
}

pub fn add_interface_allow_port(
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Allow {} {} port for interface {}",
            allowed_port,
            protocol.as_str(),
            interface
        ),
        |file| {
            let option_name = format!(
                "networking.firewall.interfaces.\"{}\".allowed{}Ports",
                interface,
                protocol.as_str()
            );
            mxList::new(&option_name, true).add(file, &allowed_port.to_string())
        },
    )
}

pub fn remove_interface_allowed_port(
    allowed_port: u32,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Remove allowed {} {} port for interface {}",
            allowed_port,
            protocol.as_str(),
            interface
        ),
        |file| {
            let option_name = format!(
                "networking.firewall.interfaces.\"{}\".allowed{}Ports",
                interface,
                protocol.as_str()
            );
            mxList::new(&option_name, true).remove(file, &allowed_port.to_string())
        },
    )
}

pub fn add_interface_allow_port_range(
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Allow {} to {} {} ports for interface {}",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str(),
            interface
        ),
        |file| {
            let option_name = format!(
                "networking.firewall.interfaces.\"{}\".allowed{}PortRanges",
                interface,
                protocol.as_str()
            );
            mxList::new(&option_name, true).add(
                file,
                &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
            )
        },
    )
}

pub fn remove_interface_allowed_port_range(
    allowed_ports: Range<u32>,
    protocol: NetworkProtocol,
    interface: &str,
) -> mx::Result<()> {
    with_firewall_transaction(
        &format!(
            "Allow {} to {} {} ports for interface {}",
            allowed_ports.start,
            allowed_ports.end,
            protocol.as_str(),
            interface
        ),
        |file| {
            let option_name = format!(
                "networking.firewall.interfaces.\"{}\".allowed{}PortRanges",
                interface,
                protocol.as_str()
            );
            mxList::new(&option_name, true).remove(
                file,
                &format!("{{from={};to={};}}", allowed_ports.start, allowed_ports.end),
            )
        },
    )
}
