//! Whether this computer can run containers at all. Linux and macOS can
//! always install a runtime. On Windows the runtimes (Docker Desktop,
//! Rancher Desktop) need WSL 2, and WSL 2 needs virtualization: a PC where
//! WSL is only turned off can turn it on (an administrator, a restart),
//! while a virtual machine without nested virtualization (an AWS
//! WorkSpace, most cloud desktops) never can.

use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Capability {
    /// "ok", "wsl-off" (WSL isn't on; the PC can turn it on) or
    /// "unsupported" (no virtualization for WSL 2).
    pub state: String,
    /// Why not, for the screen: "virtual" (a virtual machine without nested
    /// virtualization), "firmware" (turned off in the firmware settings) or
    /// "cpu" (the processor has none).
    pub reason: String,
    /// The machine as Windows names it (manufacturer, model).
    pub machine: String,
}

/// What Windows says about virtualization on this machine.
#[derive(Default, Clone, Debug)]
pub struct Facts {
    pub hypervisor: bool,
    pub firmware: bool,
    pub extensions: bool,
    pub manufacturer: String,
    pub model: String,
}

const VIRTUAL_MACHINES: [&str; 14] = [
    "virtual",
    "vmware",
    "amazon ec2",
    "xen",
    "kvm",
    "qemu",
    "virtualbox",
    "innotek",
    "google compute",
    "openstack",
    "parallels",
    "bochs",
    "nutanix",
    "cloud pc",
];

impl Facts {
    fn is_virtual_machine(&self) -> bool {
        let both = format!("{} {}", self.manufacturer, self.model).to_lowercase();
        VIRTUAL_MACHINES.iter().any(|v| both.contains(v))
    }
}

/// The decision, from the facts and whether `wsl --status` works.
pub fn decide(f: &Facts, wsl_works: bool) -> Capability {
    let machine = format!("{} {}", f.manufacturer.trim(), f.model.trim())
        .trim()
        .to_string();
    let cap = |state: &str, reason: &str| Capability {
        state: state.into(),
        reason: reason.into(),
        machine: machine.clone(),
    };
    // Virtualization the processor offers and nothing has taken yet.
    let free = f.firmware && f.extensions;
    // A hypervisor already runs: on a PC that is Hyper-V or Windows'
    // own security features (WSL 2 works alongside them); on a virtual
    // machine it is the host's, and only nested virtualization would help,
    // which then shows as `free` above or as WSL already working.
    let usable = free || (f.hypervisor && !f.is_virtual_machine());
    if !usable && !wsl_works {
        let reason = if f.is_virtual_machine() {
            "virtual"
        } else if !f.extensions && !f.hypervisor {
            "cpu"
        } else {
            "firmware"
        };
        return cap("unsupported", reason);
    }
    if wsl_works {
        cap("ok", "")
    } else {
        cap("wsl-off", "")
    }
}

#[cfg(windows)]
fn facts() -> Facts {
    use std::sync::OnceLock;
    static FACTS: OnceLock<Facts> = OnceLock::new();
    FACTS
        .get_or_init(|| {
            let script = "$c = Get-CimInstance Win32_ComputerSystem; \
                $p = Get-CimInstance Win32_Processor | Select-Object -First 1; \
                [pscustomobject]@{hypervisor=[bool]$c.HypervisorPresent; \
                firmware=[bool]$p.VirtualizationFirmwareEnabled; \
                extensions=[bool]$p.VMMonitorModeExtensions; \
                manufacturer=[string]$c.Manufacturer; model=[string]$c.Model} | ConvertTo-Json -Compress";
            let out = crate::engine::output(
                "powershell.exe",
                &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
                std::time::Duration::from_secs(30),
            );
            let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
            let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            Facts {
                hypervisor: b("hypervisor"),
                // No answer at all: don't claim the PC can't, let WSL decide.
                firmware: if out.is_empty() { true } else { b("firmware") },
                extensions: if out.is_empty() { true } else { b("extensions") },
                manufacturer: s("manufacturer"),
                model: s("model"),
            }
        })
        .clone()
}

#[cfg(windows)]
fn wsl_works() -> bool {
    let mut cmd = crate::engine::command("wsl.exe");
    cmd.arg("--status");
    crate::engine::run_capture(cmd, std::time::Duration::from_secs(20)).0 == 0
}

pub fn check() -> Capability {
    #[cfg(windows)]
    {
        decide(&facts(), wsl_works())
    }
    #[cfg(not(windows))]
    {
        decide(
            &Facts {
                firmware: true,
                extensions: true,
                ..Default::default()
            },
            true,
        )
    }
}

/// Turns WSL on (Windows asks for an administrator), without a Linux
/// distribution: the runtimes bring their own. Returns the exit status
/// and output; a restart finishes it.
pub fn enable_wsl() -> (i32, String) {
    if !cfg!(windows) {
        return (1, "WSL is a Windows feature".into());
    }
    let script =
        "try { $p = Start-Process -FilePath wsl.exe -ArgumentList '--install','--no-distribution' \
        -Verb RunAs -Wait -PassThru; exit $p.ExitCode } catch { Write-Error $_; exit 1 }";
    let mut cmd = crate::engine::command("powershell.exe");
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    crate::engine::run_capture(cmd, std::time::Duration::from_secs(1800))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(
        hypervisor: bool,
        firmware: bool,
        extensions: bool,
        manufacturer: &str,
        model: &str,
    ) -> Facts {
        Facts {
            hypervisor,
            firmware,
            extensions,
            manufacturer: manufacturer.into(),
            model: model.into(),
        }
    }

    #[test]
    fn decisions() {
        // A WorkSpace: the host's hypervisor, nothing nested.
        let ws = f(true, false, true, "Amazon EC2", "m5.large");
        assert_eq!(decide(&ws, false).state, "unsupported");
        assert_eq!(decide(&ws, false).reason, "virtual");
        assert_eq!(decide(&ws, false).machine, "Amazon EC2 m5.large");
        // A PC with memory integrity on (a hypervisor of its own), WSL off.
        assert_eq!(
            decide(&f(true, false, true, "Dell Inc.", "XPS 15"), false).state,
            "wsl-off"
        );
        // A PC with virtualization free, WSL off; and on.
        assert_eq!(
            decide(&f(false, true, true, "LENOVO", "20XW"), false).state,
            "wsl-off"
        );
        assert_eq!(
            decide(&f(false, true, true, "LENOVO", "20XW"), true).state,
            "ok"
        );
        // Turned off in the firmware.
        let off = decide(&f(false, false, true, "HP", "EliteBook"), false);
        assert_eq!(
            (off.state.as_str(), off.reason.as_str()),
            ("unsupported", "firmware")
        );
        // A VM with nested virtualization, and one where WSL already works.
        assert_eq!(
            decide(
                &f(true, true, true, "Microsoft Corporation", "Virtual Machine"),
                false
            )
            .state,
            "wsl-off"
        );
        assert_eq!(
            decide(
                &f(
                    true,
                    false,
                    true,
                    "Microsoft Corporation",
                    "Virtual Machine"
                ),
                true
            )
            .state,
            "ok"
        );
        // No virtualization in the processor.
        assert_eq!(
            decide(&f(false, false, false, "Acme", "Box"), false).reason,
            "cpu"
        );
        // This machine (in CI, what a GitHub runner reports).
        let here = check();
        println!("containers here: {here:?}");
        if !cfg!(windows) {
            assert_eq!(here.state, "ok");
        }
    }
}
