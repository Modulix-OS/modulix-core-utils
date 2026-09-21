//! Opens and closes firewall ports in the configuration's `firewall.nix`.
//!
//! Four axes multiply into the API: a single port or a port range, globally or
//! on one interface, added or removed. Each operation exists in a
//! `*_no_transaction` form editing an already-open file, and in a wrapper
//! opening its own transaction and rebuilding.

use std::ops::Range;

use crate::core::list::List as mxList;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::{BuildCommand, UpdateInput};
use crate::{core::transaction, mx};

/// Transport protocol a firewall rule applies to.
///
/// # Variants
/// * `Udp` - UDP traffic.
/// * `Tcp` - TCP traffic.
pub enum NetworkProtocol {
    Udp,
    Tcp,
}

/// Configuration file, relative to the config directory, holding the
/// `networking.firewall` options.
const FILE_FIREWALL_PATH: &str = "firewall.nix";

impl NetworkProtocol {
    /// Protocol name as NixOS spells it inside an option name.
    ///
    /// # Returns
    /// `"TCP"` or `"UDP"`, to be interpolated into
    /// `allowed<proto>Ports`/`allowed<proto>PortRanges`.
    pub fn as_str(&self) -> &str {
        match self {
            NetworkProtocol::Tcp => "TCP",
            NetworkProtocol::Udp => "UDP",
        }
    }
}

/// Opens one port on every interface, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_port` - port number to add to
///   `networking.firewall.allowed<proto>Ports`.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// The option is declared if absent, and the port is added only once:
/// re-adding an already-open port changes nothing.
pub fn add_global_allow_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
    mxList::new(&option_name, true).add(file, &allowed_port.to_string())?;
    Ok(())
}

/// Closes one globally open port, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_port` - port number to remove.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// A port that is not open is silently ignored; the option is dropped when its
/// last port goes. A port opened on a specific interface is left alone.
pub fn remove_global_allowed_port_no_transaction(
    file: &mut NixFile,
    allowed_port: u32,
    protocol: NetworkProtocol,
) -> mx::Result<()> {
    let option_name = format!("networking.firewall.allowed{}Ports", protocol.as_str());
    mxList::new(&option_name, true).remove(file, &allowed_port.to_string())?;
    Ok(())
}

/// Opens a port range on every interface, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_ports` - range to open, written as `{from=start;to=end;}`. Both
///   bounds end up inclusive in NixOS, so the Rust half-open convention does
///   *not* hold: pass the last port you want open as `end`.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// The option is declared if absent, and an identical range is not added
/// twice. Overlapping ranges are kept as they are, not merged.
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

/// Closes a globally open port range, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_ports` - range to remove; it must match an existing declaration
///   exactly, since entries are compared as text.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// A range that is not declared is silently ignored; a range that merely
/// overlaps a declared one is not touched.
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

/// Opens one port on a single interface, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_port` - port number to add to
///   `networking.firewall.interfaces."<interface>".allowed<proto>Ports`.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface name (e.g. `eth0`), used as the attribute key.
///
/// # Post-conditions
/// The per-interface option is declared if absent, and the port is added only
/// once. Nothing changes for the other interfaces.
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

/// Closes one port on a single interface, in an already-open `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_port` - port number to remove.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule belongs to.
///
/// # Post-conditions
/// A port that is not open on this interface is silently ignored. A globally
/// open port of the same number stays open.
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

/// Opens a port range on a single interface, in an already-open
/// `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_ports` - range to open; both bounds are inclusive for NixOS, so
///   `end` must be the last port to open.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule belongs to.
///
/// # Post-conditions
/// The per-interface option is declared if absent, and an identical range is
/// not added twice.
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

/// Closes a port range on a single interface, in an already-open
/// `firewall.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `allowed_ports` - range to remove; must match an existing declaration
///   exactly.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule belongs to.
///
/// # Post-conditions
/// A range that is not declared on this interface is silently ignored.
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

/// Opens one port on every interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit (see
///   [`crate::CONFIG_DIRECTORY`]).
/// * `allowed_port` - port number to open.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// On success the rule is part of the active generation and the firewall is
/// live; on error the configuration is rolled back. Blocks for the whole
/// `nixos-rebuild switch`.
///
/// # Errors
/// [`mx::ErrorKind::BuildError`] if the rebuild fails, plus any error from
/// editing the file.
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
        UpdateInput::Keep,
        |file| add_global_allow_port_no_transaction(file, allowed_port, protocol),
    )
}

/// Closes one globally open port and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_port` - port number to close.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`]; a port that was not open is not an error,
/// and the rebuild runs anyway.
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
        UpdateInput::Keep,
        |file| remove_global_allowed_port_no_transaction(file, allowed_port, protocol),
    )
}

/// Opens a port range on every interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_ports` - range to open; `end` is inclusive for NixOS.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`].
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
        UpdateInput::Keep,
        |file| add_global_allowed_port_range_no_transaction(file, allowed_ports, protocol),
    )
}

/// Closes a globally open port range and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_ports` - range to remove; must match an existing declaration
///   exactly.
/// * `protocol` - protocol the rule applies to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`].
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
        UpdateInput::Keep,
        |file| remove_global_allowed_port_range_no_transaction(file, allowed_ports, protocol),
    )
}

/// Opens one port on a single interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_port` - port number to open.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule applies to; it is not checked against
///   the interfaces the machine actually has.
///
/// # Post-conditions
/// As in [`add_global_allow_port`].
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
        UpdateInput::Keep,
        |file| add_interface_allow_port_no_transaction(file, allowed_port, protocol, interface),
    )
}

/// Closes one port on a single interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_port` - port number to close.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule belongs to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`]; a globally open port of the same number
/// stays open.
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
        UpdateInput::Keep,
        |file| {
            remove_interface_allowed_port_no_transaction(file, allowed_port, protocol, interface)
        },
    )
}

/// Opens a port range on a single interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_ports` - range to open; `end` is inclusive for NixOS.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule applies to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`].
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
        UpdateInput::Keep,
        |file| {
            add_interface_allow_port_range_no_transaction(file, allowed_ports, protocol, interface)
        },
    )
}

/// Closes a port range on a single interface and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `allowed_ports` - range to remove; must match an existing declaration
///   exactly.
/// * `protocol` - protocol the rule applies to.
/// * `interface` - interface the rule belongs to.
///
/// # Post-conditions
/// As in [`add_global_allow_port`].
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
        UpdateInput::Keep,
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
