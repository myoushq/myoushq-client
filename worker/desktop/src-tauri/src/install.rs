//! Getting a container runtime, and the one fix for each way an installed
//! one can't be used yet. Which runtimes this system can have, in the order
//! the Set up card offers them, and for each whether the app can install it
//! itself or opens its download page: the app installs only where the tool
//! for it is there (winget on Windows, Homebrew on a Mac, apt, dnf or pacman
//! through pkexec on Linux), never offering what can't work here. The same
//! for starting Docker Engine (pkexec and systemd), and adding compose.
//! Commands are argument lists built here; the window only names them.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

/// What this computer has that the app can install or start things with.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Tools {
    /// "linux", "windows" or "macos".
    pub os: String,
    /// Full paths of the tools found, empty when missing.
    pub winget: String,
    pub brew: String,
    pub pkexec: String,
    /// systemd runs this session's system (`systemctl` works).
    pub systemd: bool,
    /// The package manager: "apt", "dnf", "pacman" or "".
    pub pm: String,
    /// os-release's ID and ID_LIKE ("ubuntu", "debian").
    pub distro: String,
    pub like: String,
    /// The compose packages apt knows here, in the order to prefer them.
    pub apt_compose: Vec<String>,
    /// The owner's login name.
    pub user: String,
}

impl Tools {
    fn is(&self, family: &str) -> bool {
        self.distro == family || self.like.split_whitespace().any(|l| l == family)
    }
}

/// The compose packages apt may have: Docker's own repository's plugin,
/// Ubuntu's, then Debian's (version 2 from Debian 13).
const APT_COMPOSE: [&str; 3] = [
    "docker-compose-plugin",
    "docker-compose-v2",
    "docker-compose",
];

/// Finds the tools on this computer. `apt-cache` is asked which compose
/// package it knows, so this takes a moment: call it from a thread.
pub fn detect() -> Tools {
    let which = |n: &str| {
        crate::engine::which(n)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    let os_release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    let systemd = Path::new("/run/systemd/system").is_dir();
    // Known, and for Debian's docker-compose only version 2 (version 1,
    // in Debian 12, is a separate program, not `docker compose`).
    let apt_knows = |pkg: &str| {
        let mut cmd = crate::engine::command("apt-cache");
        cmd.args(["show", "--no-all-versions", pkg]);
        let (status, out) = crate::engine::run_capture(cmd, Duration::from_secs(15));
        status == 0 && (pkg != "docker-compose" || out.lines().any(|l| l.starts_with("Version: 2")))
    };
    detect_with(
        crate::platform::os(),
        &which,
        &os_release,
        systemd,
        &apt_knows,
    )
}

/// `detect` with what it reads passed in, for the tests.
pub fn detect_with(
    os: &str,
    which: &dyn Fn(&str) -> String,
    os_release: &str,
    systemd: bool,
    apt_knows: &dyn Fn(&str) -> bool,
) -> Tools {
    let mut t = Tools {
        os: os.into(),
        user: std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default(),
        ..Default::default()
    };
    match os {
        "windows" => t.winget = which("winget"),
        "macos" => {
            t.brew = which("brew");
            // An app opened from the Finder has a short PATH.
            for p in ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"] {
                if t.brew.is_empty() && Path::new(p).is_file() {
                    t.brew = p.into();
                }
            }
        }
        _ => {
            t.pkexec = which("pkexec");
            t.systemd = systemd && !which("systemctl").is_empty();
            for (pm, cmd) in [("apt", "apt-get"), ("dnf", "dnf"), ("pacman", "pacman")] {
                if t.pm.is_empty() && !which(cmd).is_empty() {
                    t.pm = pm.into();
                }
            }
            for line in os_release.lines() {
                let value = |k: &str| {
                    line.strip_prefix(k)
                        .map(|v| v.trim().trim_matches('"').to_lowercase())
                };
                if let Some(v) = value("ID=") {
                    t.distro = v;
                }
                if let Some(v) = value("ID_LIKE=") {
                    t.like = v;
                }
            }
            if t.pm == "apt" {
                t.apt_compose = APT_COMPOSE
                    .iter()
                    .filter(|p| apt_knows(p))
                    .map(|p| p.to_string())
                    .collect();
            }
        }
    }
    t
}

/// One runtime the card offers.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Runtime {
    /// What the window asks to install: "docker-engine", "podman",
    /// "docker-desktop", "rancher-desktop", "orbstack".
    pub id: String,
    pub name: String,
    /// Its download or install page.
    pub url: String,
    /// What the app's Install runs, as it runs it (on Linux as root through
    /// pkexec, so without sudo); empty when the app can't install it here
    /// (the card then offers only the page).
    pub install: String,
    #[serde(skip)]
    pub argv: Vec<String>,
}

/// What the Set up card can offer on this computer.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// In the order the card offers them.
    pub runtimes: Vec<Runtime>,
    /// What Install uses, for the card's hint: "winget", "Homebrew",
    /// "pkexec" or "" when the app installs nothing here.
    pub tool: String,
    /// Starting Docker Engine on Linux: what the app's button runs, as root
    /// through pkexec (empty when it can't: no pkexec or no systemd), and
    /// the line to type in a terminal (with sudo).
    pub start: String,
    #[serde(skip)]
    pub start_argv: Vec<String>,
    pub start_line: String,
    /// The commands that let the owner use the runtime (then log out and in).
    pub group_lines: Vec<String>,
    /// Adding compose: what the app's button runs, as root through pkexec
    /// (empty when it can't), the line to type (with sudo), and where it's
    /// explained.
    pub compose: String,
    #[serde(skip)]
    pub compose_argv: Vec<String>,
    pub compose_line: String,
    pub compose_url: String,
}

const DOCKER_DESKTOP: &str = "https://www.docker.com/products/docker-desktop/";

fn runtime(id: &str, name: &str, url: &str, argv: Vec<String>, line: String) -> Runtime {
    Runtime {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        install: if argv.is_empty() { String::new() } else { line },
        argv,
    }
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Runs a shell line as root through pkexec, the owner's name as `$1`
/// (never pasted into the line).
fn pkexec_sh(t: &Tools, line: &str) -> Vec<String> {
    vec![
        t.pkexec.clone(),
        "sh".into(),
        "-c".into(),
        line.into(),
        "sh".into(),
        t.user.clone(),
    ]
}

/// What the card offers here. `runtime` is the runtime found, if any
/// ("Podman" asks for podman-compose, not Docker's).
pub fn options(t: &Tools, runtime_name: &str) -> Options {
    let mut o = Options::default();
    match t.os.as_str() {
        "windows" => {
            let winget = |id: &str| {
                if t.winget.is_empty() {
                    return (vec![], String::new());
                }
                let mut argv = vec![t.winget.clone()];
                argv.extend(strings(&[
                    "install",
                    "--exact",
                    "--id",
                    id,
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ]));
                (argv, format!("winget install --exact --id {id}"))
            };
            let (a, l) = winget("Docker.DockerDesktop");
            o.runtimes.push(runtime(
                "docker-desktop",
                "Docker Desktop",
                DOCKER_DESKTOP,
                a,
                l,
            ));
            let (a, l) = winget("SUSE.RancherDesktop");
            o.runtimes.push(runtime(
                "rancher-desktop",
                "Rancher Desktop",
                "https://rancherdesktop.io/",
                a,
                l,
            ));
            if !t.winget.is_empty() {
                o.tool = "winget".into();
            }
            let user = if t.user.is_empty() {
                "%USERNAME%"
            } else {
                &t.user
            };
            o.group_lines = vec![format!("net localgroup docker-users \"{user}\" /add")];
            o.compose_url = "https://docs.docker.com/compose/install/".into();
        }
        "macos" => {
            let brew = |cask: &str| {
                if t.brew.is_empty() {
                    return (vec![], String::new());
                }
                (
                    vec![
                        t.brew.clone(),
                        "install".into(),
                        "--cask".into(),
                        cask.into(),
                    ],
                    format!("brew install --cask {cask}"),
                )
            };
            let (a, l) = brew("docker-desktop");
            o.runtimes.push(runtime(
                "docker-desktop",
                "Docker Desktop",
                DOCKER_DESKTOP,
                a,
                l,
            ));
            let (a, l) = brew("orbstack");
            o.runtimes.push(runtime(
                "orbstack",
                "OrbStack",
                "https://orbstack.dev/",
                a,
                l,
            ));
            if !t.brew.is_empty() {
                o.tool = "Homebrew".into();
                o.compose_argv = vec![t.brew.clone(), "install".into(), "docker-compose".into()];
                o.compose = "brew install docker-compose".into();
            }
            o.compose_line = "brew install docker-compose".into();
            o.compose_url = "https://docs.docker.com/compose/install/".into();
        }
        _ => linux(t, runtime_name, &mut o),
    }
    o
}

fn linux(t: &Tools, runtime_name: &str, o: &mut Options) {
    let root = !t.pkexec.is_empty();
    let enable = if t.systemd {
        " && systemctl enable --now docker"
    } else {
        ""
    };
    // Docker Engine from the distribution, started, and the owner in the
    // docker group, in one password prompt.
    let compose_apt = t.apt_compose.first().cloned();
    let engine: Option<String> = match t.pm.as_str() {
        "apt" if t.is("debian") || t.is("ubuntu") => Some(format!(
            "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io{}",
            compose_apt
                .as_deref()
                .map(|p| format!(" {p}"))
                .unwrap_or_default()
        )),
        "dnf" if t.is("fedora") => Some("dnf install -y moby-engine docker-compose".into()),
        "pacman" if t.is("arch") => {
            Some("pacman -S --noconfirm --needed docker docker-compose".into())
        }
        _ => None,
    };
    let podman: Option<String> = match t.pm.as_str() {
        "apt" if t.is("debian") || t.is("ubuntu") => Some(
            "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y podman podman-compose"
                .into(),
        ),
        "dnf" if t.is("fedora") => Some("dnf install -y podman podman-compose".into()),
        "pacman" if t.is("arch") => {
            Some("pacman -S --noconfirm --needed podman podman-compose".into())
        }
        _ => None,
    };
    // What the Install button runs, as it runs it (as root through
    // pkexec, which asks for the password): no sudo in it.
    let shown_line = |line: &str| line.replace("DEBIAN_FRONTEND=noninteractive ", "");
    match engine.filter(|_| root) {
        Some(line) => {
            let full = format!("{line}{enable} && usermod -aG docker \"$1\"");
            let shown = format!("{line}{enable} && usermod -aG docker {}", t.user);
            o.runtimes.push(runtime(
                "docker-engine",
                "Docker Engine",
                "https://docs.docker.com/engine/install/",
                pkexec_sh(t, &full),
                shown_line(&shown),
            ));
        }
        None => o.runtimes.push(runtime(
            "docker-engine",
            "Docker Engine",
            "https://docs.docker.com/engine/install/",
            vec![],
            String::new(),
        )),
    }
    match podman.filter(|_| root) {
        Some(line) => o.runtimes.push(runtime(
            "podman",
            "Podman",
            "https://podman.io/docs/installation",
            pkexec_sh(t, &line),
            shown_line(&line),
        )),
        None => o.runtimes.push(runtime(
            "podman",
            "Podman",
            "https://podman.io/docs/installation",
            vec![],
            String::new(),
        )),
    }
    o.runtimes.push(runtime(
        "docker-desktop",
        "Docker Desktop",
        DOCKER_DESKTOP,
        vec![],
        String::new(),
    ));
    if o.runtimes.iter().any(|r| !r.argv.is_empty()) {
        o.tool = "pkexec".into();
    }
    // Docker Engine's service: a button where pkexec and systemd are there.
    o.start_line = if t.systemd {
        "sudo systemctl enable --now docker".into()
    } else {
        "sudo service docker start".into()
    };
    if root && t.systemd {
        o.start_argv = vec![
            t.pkexec.clone(),
            "systemctl".into(),
            "enable".into(),
            "--now".into(),
            "docker".into(),
        ];
        o.start = "systemctl enable --now docker".into();
    }
    o.group_lines = vec!["sudo usermod -aG docker $USER".into()];
    // Compose: Podman's own, else the distribution's package for Docker's.
    let package = if runtime_name == "Podman" {
        Some("podman-compose".to_string())
    } else {
        match t.pm.as_str() {
            "apt" => compose_apt,
            "dnf" | "pacman" => Some("docker-compose".into()),
            _ => None,
        }
    };
    o.compose_url = if runtime_name == "Podman" {
        "https://github.com/containers/podman-compose".into()
    } else {
        "https://docs.docker.com/compose/install/linux/".into()
    };
    if let Some(pkg) = package {
        let (argv, line): (Vec<String>, String) = match t.pm.as_str() {
            "apt" => (
                strings(&["apt-get", "install", "-y", &pkg]),
                format!("sudo apt-get install {pkg}"),
            ),
            "dnf" => (
                strings(&["dnf", "install", "-y", &pkg]),
                format!("sudo dnf install {pkg}"),
            ),
            "pacman" => (
                strings(&["pacman", "-S", "--noconfirm", "--needed", &pkg]),
                format!("sudo pacman -S {pkg}"),
            ),
            _ => (vec![], String::new()),
        };
        o.compose_line = line;
        if root && !argv.is_empty() {
            o.compose = argv.join(" ");
            o.compose_argv = [vec![t.pkexec.clone()], argv].concat();
        }
    }
}

/// The command for what the window asked: a runtime's id, "start" or
/// "compose". None when the app can't do it here.
pub fn command(o: &Options, id: &str) -> Option<Vec<String>> {
    let argv = match id {
        "start" => o.start_argv.clone(),
        "compose" => o.compose_argv.clone(),
        _ => o.runtimes.iter().find(|r| r.id == id)?.argv.clone(),
    };
    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn which_of(found: &'static [&'static str]) -> impl Fn(&str) -> String {
        move |n: &str| {
            if found.contains(&n) {
                format!("/usr/bin/{n}")
            } else {
                String::new()
            }
        }
    }

    const UBUNTU: &str = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n";
    const DEBIAN: &str = "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nID=debian\n";
    const FEDORA: &str = "NAME=\"Fedora Linux\"\nID=fedora\n";
    const MINT: &str = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
    const OPENSUSE: &str = "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";

    fn ubuntu(found: &'static [&'static str]) -> Tools {
        let mut t = detect_with("linux", &which_of(found), UBUNTU, true, &|p: &str| {
            p == "docker-compose-v2"
        });
        t.user = "max".into();
        t
    }

    #[test]
    fn detects_the_tools() {
        let t = ubuntu(&["pkexec", "systemctl", "apt-get"]);
        assert_eq!(
            (
                t.pkexec.as_str(),
                t.systemd,
                t.pm.as_str(),
                t.distro.as_str(),
                t.like.as_str()
            ),
            ("/usr/bin/pkexec", true, "apt", "ubuntu", "debian")
        );
        assert_eq!(t.apt_compose, vec!["docker-compose-v2"]);
        // No systemctl: no systemd, whatever /run says (a container, WSL 1).
        assert!(!ubuntu(&["pkexec", "apt-get"]).systemd);
        let mint = detect_with("linux", &which_of(&["apt-get"]), MINT, false, &|_| false);
        assert!(mint.is("ubuntu") && mint.is("debian") && !mint.is("fedora"));
        let w = detect_with("windows", &which_of(&["winget"]), "", false, &|_| true);
        assert_eq!(
            (w.winget.as_str(), w.pkexec.as_str(), w.pm.as_str()),
            ("/usr/bin/winget", "", "")
        );
        let none = detect_with("windows", &which_of(&[]), "", false, &|_| true);
        assert_eq!(none.winget, "");
        let mac = detect_with("macos", &which_of(&["brew"]), "", false, &|_| true);
        assert_eq!(mac.brew, "/usr/bin/brew");
    }

    /// Detection reads PATH: a folder with the tools in it finds them.
    #[test]
    fn finds_tools_on_a_path() {
        let dir = std::env::temp_dir().join(format!("myous-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = if cfg!(windows) { ".exe" } else { "" };
        for n in ["winget", "pkexec", "apt-get"] {
            std::fs::write(dir.join(format!("{n}{exe}")), "").unwrap();
        }
        let path = std::env::join_paths([&dir]).unwrap();
        let which = |n: &str| {
            crate::engine::which_in(n, &path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let linux = detect_with("linux", &which, UBUNTU, true, &|_| false);
        assert_eq!(
            linux.pkexec,
            dir.join(format!("pkexec{exe}")).to_string_lossy()
        );
        assert_eq!((linux.pm.as_str(), linux.systemd), ("apt", false));
        let windows = detect_with("windows", &which, "", false, &|_| false);
        assert_eq!(
            windows.winget,
            dir.join(format!("winget{exe}")).to_string_lossy()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn ids(o: &Options) -> Vec<(&str, bool)> {
        o.runtimes
            .iter()
            .map(|r| (r.id.as_str(), !r.install.is_empty()))
            .collect()
    }

    #[test]
    fn windows_installs_with_winget_only_where_it_is() {
        let mut t = Tools {
            os: "windows".into(),
            user: "max".into(),
            ..Default::default()
        };
        let o = options(&t, "");
        assert_eq!(
            ids(&o),
            vec![("docker-desktop", false), ("rancher-desktop", false)]
        );
        assert_eq!(o.tool, "");
        assert_eq!(command(&o, "docker-desktop"), None);
        t.winget = r"C:\Users\max\AppData\Local\Microsoft\WindowsApps\winget.exe".into();
        let o = options(&t, "");
        assert_eq!(
            ids(&o),
            vec![("docker-desktop", true), ("rancher-desktop", true)]
        );
        assert_eq!(o.tool, "winget");
        assert_eq!(
            o.runtimes[0].install,
            "winget install --exact --id Docker.DockerDesktop"
        );
        let argv = command(&o, "rancher-desktop").unwrap();
        assert_eq!(argv[0], t.winget);
        assert!(argv.contains(&"SUSE.RancherDesktop".to_string()));
        assert_eq!(
            o.group_lines,
            vec!["net localgroup docker-users \"max\" /add"]
        );
        assert_eq!((o.start.as_str(), command(&o, "start")), ("", None));
    }

    #[test]
    fn a_mac_installs_with_homebrew_only_where_it_is() {
        let mut t = Tools {
            os: "macos".into(),
            ..Default::default()
        };
        assert_eq!(
            ids(&options(&t, "")),
            vec![("docker-desktop", false), ("orbstack", false)]
        );
        t.brew = "/opt/homebrew/bin/brew".into();
        let o = options(&t, "");
        assert_eq!(ids(&o), vec![("docker-desktop", true), ("orbstack", true)]);
        assert_eq!(
            command(&o, "orbstack").unwrap(),
            vec!["/opt/homebrew/bin/brew", "install", "--cask", "orbstack"]
        );
        assert_eq!(o.runtimes[0].install, "brew install --cask docker-desktop");
    }

    #[test]
    fn linux_installs_through_pkexec_with_the_distributions_packages() {
        // Ubuntu with pkexec and systemd: Docker Engine and Podman installable, Docker Desktop a page.
        let t = ubuntu(&["pkexec", "systemctl", "apt-get"]);
        let o = options(&t, "");
        assert_eq!(
            ids(&o),
            vec![
                ("docker-engine", true),
                ("podman", true),
                ("docker-desktop", false)
            ]
        );
        assert_eq!(o.tool, "pkexec");
        let argv = command(&o, "docker-engine").unwrap();
        assert_eq!(&argv[..3], &["/usr/bin/pkexec", "sh", "-c"]);
        assert_eq!(
            argv[3],
            "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io docker-compose-v2 && systemctl enable --now docker && usermod -aG docker \"$1\""
        );
        assert_eq!(
            &argv[4..],
            &["sh", "max"],
            "the owner's name is an argument, never in the line"
        );
        assert_eq!(
            o.runtimes[0].install,
            "apt-get update && apt-get install -y docker.io docker-compose-v2 && systemctl enable --now docker && usermod -aG docker max"
        );
        assert_eq!(
            command(&o, "start").unwrap(),
            vec!["/usr/bin/pkexec", "systemctl", "enable", "--now", "docker"]
        );
        // What the buttons run is shown as they run it (through pkexec, no sudo);
        // the lines to type in a terminal carry sudo.
        assert_eq!(o.start, "systemctl enable --now docker");
        assert_eq!(o.compose, "apt-get install -y docker-compose-v2");
        assert!(o.runtimes.iter().all(|r| !r.install.contains("sudo")));
        assert_eq!(
            command(&o, "compose").unwrap(),
            vec![
                "/usr/bin/pkexec",
                "apt-get",
                "install",
                "-y",
                "docker-compose-v2"
            ]
        );
        assert_eq!(o.compose_line, "sudo apt-get install docker-compose-v2");
        assert_eq!(o.group_lines, vec!["sudo usermod -aG docker $USER"]);
        assert_eq!(command(&o, "docker-desktop"), None);
        // Docker's own repository: its plugin, not Ubuntu's.
        let mut docker_repo = t.clone();
        docker_repo.apt_compose = vec!["docker-compose-plugin".into(), "docker-compose-v2".into()];
        assert_eq!(
            options(&docker_repo, "Docker Engine").compose_line,
            "sudo apt-get install docker-compose-plugin"
        );
        // Podman's compose is its own.
        assert_eq!(
            options(&t, "Podman").compose_line,
            "sudo apt-get install podman-compose"
        );
    }

    #[test]
    fn linux_without_pkexec_or_systemd_shows_the_lines() {
        // No pkexec: every runtime is a page, the service and compose are lines to type.
        let t = ubuntu(&["systemctl", "apt-get"]);
        let o = options(&t, "Docker Engine");
        assert_eq!(
            ids(&o),
            vec![
                ("docker-engine", false),
                ("podman", false),
                ("docker-desktop", false)
            ]
        );
        assert_eq!(
            (o.tool.as_str(), o.start.as_str(), o.compose.as_str()),
            ("", "", "")
        );
        assert_eq!(o.start_line, "sudo systemctl enable --now docker");
        assert_eq!(o.compose_line, "sudo apt-get install docker-compose-v2");
        assert_eq!(command(&o, "start"), None);
        // pkexec but no systemd: installs still work (without enabling the service), starting is a line.
        let t = ubuntu(&["pkexec", "apt-get"]);
        let o = options(&t, "");
        assert!(!command(&o, "docker-engine").unwrap()[3].contains("systemctl"));
        assert_eq!(
            (o.start.as_str(), o.start_line.as_str()),
            ("", "sudo service docker start")
        );
        // Debian 12: no v2 compose package at all; Docker Engine still installs, compose is a page.
        let deb = detect_with(
            "linux",
            &which_of(&["pkexec", "systemctl", "apt-get"]),
            DEBIAN,
            true,
            &|_| false,
        );
        let o = options(&deb, "Docker Engine");
        assert!(command(&o, "docker-engine").unwrap()[3].contains("install -y docker.io &&"));
        assert_eq!((o.compose.as_str(), o.compose_line.as_str()), ("", ""));
        assert_eq!(
            o.compose_url,
            "https://docs.docker.com/compose/install/linux/"
        );
        // Fedora and Arch with their own packages; an unknown distribution gets the pages.
        let fed = detect_with(
            "linux",
            &which_of(&["pkexec", "systemctl", "dnf"]),
            FEDORA,
            true,
            &|_| false,
        );
        assert!(command(&options(&fed, ""), "docker-engine").unwrap()[3]
            .starts_with("dnf install -y moby-engine docker-compose"));
        let suse = detect_with(
            "linux",
            &which_of(&["pkexec", "systemctl"]),
            OPENSUSE,
            true,
            &|_| false,
        );
        assert_eq!(
            ids(&options(&suse, "")),
            vec![
                ("docker-engine", false),
                ("podman", false),
                ("docker-desktop", false)
            ]
        );
    }

    /// The tools on this machine (in CI, the runner's), printed.
    #[test]
    fn detects_here() {
        let t = detect();
        println!("tools here: {t:?}\noptions: {:?}", options(&t, ""));
        assert_eq!(t.os, crate::platform::os());
    }
}
