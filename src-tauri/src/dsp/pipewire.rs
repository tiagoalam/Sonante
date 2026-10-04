use super::DspError;
use serde_json::Value;
use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub serial: Option<u64>, // Snapshot only; never persisted.
    pub name: String,
    pub media_class: String,
    pub autoconnect: Option<bool>,
    pub virtual_sink: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub node_id: u32,
    pub name: String,
    pub channel: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: u32,
    pub output_port: u32,
    pub input_port: u32,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub ports: Vec<Port>,
    pub links: Vec<Link>,
}

fn number(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn prop_u32(props: &Value, name: &str) -> Option<u32> {
    number(&props[name])
}

pub fn parse_snapshot(bytes: &[u8]) -> Result<Graph, DspError> {
    let objects: Vec<Value> = serde_json::from_slice(bytes)
        .map_err(|_| DspError::SnapshotFailed("invalid pw-dump JSON".into()))?;
    let mut graph = Graph::default();
    for object in objects {
        let Some(id) = number(&object["id"]) else {
            continue;
        };
        let props = &object["info"]["props"];
        match object["type"].as_str() {
            Some("PipeWire:Interface:Node") => {
                let (Some(name), Some(media_class)) =
                    (props["node.name"].as_str(), props["media.class"].as_str())
                else {
                    continue;
                };
                graph.nodes.push(Node {
                    id,
                    serial: props["object.serial"]
                        .as_u64()
                        .or_else(|| props["object.serial"].as_str().and_then(|s| s.parse().ok())),
                    name: name.into(),
                    media_class: media_class.into(),
                    autoconnect: props["node.autoconnect"].as_bool().or_else(|| {
                        props["node.autoconnect"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                    }),
                    virtual_sink: props["node.virtual"] == true
                        || props["node.virtual"] == "true"
                        || props["factory.name"] == "support.null-audio-sink",
                });
            }
            Some("PipeWire:Interface:Port") => {
                let (Some(node_id), Some(name), Some(direction)) = (
                    prop_u32(props, "node.id"),
                    props["port.name"].as_str(),
                    props["port.direction"].as_str(),
                ) else {
                    continue;
                };
                let direction = match direction {
                    "in" => Direction::Input,
                    "out" => Direction::Output,
                    _ => continue,
                };
                let channel = props["audio.channel"].as_str().unwrap_or("");
                graph.ports.push(Port {
                    id,
                    node_id,
                    name: name.into(),
                    channel: channel.into(),
                    direction,
                });
            }
            Some("PipeWire:Interface:Link") => {
                if let (Some(output_port), Some(input_port)) = (
                    prop_u32(props, "link.output.port"),
                    prop_u32(props, "link.input.port"),
                ) {
                    graph.links.push(Link {
                        id,
                        output_port,
                        input_port,
                        active: object["info"]["state"] == "active",
                    });
                }
            }
            _ => {}
        }
    }
    Ok(graph)
}

pub fn exact_node<'a>(
    graph: &'a Graph,
    name: &str,
    media_class: &str,
) -> Result<&'a Node, DspError> {
    let candidates: Vec<_> = graph.nodes.iter().filter(|n| n.name == name).collect();
    match candidates.as_slice() {
        [] => Err(DspError::NodeMissing(name.into())),
        [node] if node.media_class == media_class => Ok(node),
        [_] => Err(DspError::TopologyInvalid(vec![
            RouteViolation::WrongNodeClass(name.into()),
        ])),
        _ => Err(DspError::NodeAmbiguous(name.into())),
    }
}

pub fn stereo_ports<'a>(
    graph: &'a Graph,
    node: &Node,
    direction: Direction,
) -> Result<[&'a Port; 2], DspError> {
    let ports: Vec<_> = graph
        .ports
        .iter()
        .filter(|p| p.node_id == node.id)
        .collect();
    if ports.len() != 2 || ports.iter().any(|p| p.direction != direction) {
        return Err(DspError::PortMissing(node.name.clone()));
    }
    let channel = |name: &str| -> Result<&'a Port, DspError> {
        let matches: Vec<_> = ports
            .iter()
            .copied()
            .filter(|p| p.channel == name)
            .collect();
        match matches.as_slice() {
            [port] => Ok(*port),
            _ => Err(DspError::PortMissing(format!("{} {name}", node.name))),
        }
    };
    Ok([channel("FL")?, channel("FR")?])
}

#[derive(Debug, Clone)]
pub struct DspRoute {
    pub mpd_name: Option<String>,
    pub capture_name: String,
    pub playback_name: String,
    pub sink_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteViolation {
    MissingNode(String),
    DuplicateNode(String),
    WrongNodeClass(String),
    AutoconnectEnabled(String),
    PortInvalid(String),
    MissingLink(u32, u32),
    ExtraLink(u32),
    DuplicateLink(u32, u32),
    InactiveLink(u32),
    MissingMpdStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteStatus {
    NotReady,
    CamillaReady,
    RouteReady,
    Invalid(Vec<RouteViolation>),
}

fn node_for<'a>(
    graph: &'a Graph,
    name: &str,
    class: &str,
    violations: &mut Vec<RouteViolation>,
) -> Option<&'a Node> {
    match exact_node(graph, name, class) {
        Ok(n) => Some(n),
        Err(DspError::NodeMissing(_)) => {
            violations.push(RouteViolation::MissingNode(name.into()));
            None
        }
        Err(DspError::NodeAmbiguous(_)) => {
            violations.push(RouteViolation::DuplicateNode(name.into()));
            None
        }
        Err(_) => {
            violations.push(RouteViolation::WrongNodeClass(name.into()));
            None
        }
    }
}

fn ports_for<'a>(
    graph: &'a Graph,
    node: Option<&Node>,
    dir: Direction,
    violations: &mut Vec<RouteViolation>,
) -> Option<[&'a Port; 2]> {
    node.and_then(|n| match stereo_ports(graph, n, dir) {
        Ok(p) => Some(p),
        Err(_) => {
            violations.push(RouteViolation::PortInvalid(n.name.clone()));
            None
        }
    })
}

pub fn validate_topology(graph: &Graph, route: &DspRoute, require_mpd: bool) -> RouteStatus {
    if require_mpd
        && route
            .mpd_name
            .as_deref()
            .is_none_or(|name| !graph.nodes.iter().any(|node| node.name == name))
    {
        return RouteStatus::NotReady;
    }
    let mut violations = Vec::new();
    let capture = node_for(
        graph,
        &route.capture_name,
        "Stream/Input/Audio",
        &mut violations,
    );
    let playback = node_for(
        graph,
        &route.playback_name,
        "Stream/Output/Audio",
        &mut violations,
    );
    let sink = node_for(graph, &route.sink_name, "Audio/Sink", &mut violations);
    let mpd = route
        .mpd_name
        .as_deref()
        .and_then(|name| node_for(graph, name, "Stream/Output/Audio", &mut violations));
    if require_mpd && route.mpd_name.is_none() {
        violations.push(RouteViolation::MissingMpdStream);
    }
    for node in [capture, playback, mpd].into_iter().flatten() {
        if node.autoconnect != Some(false) {
            violations.push(RouteViolation::AutoconnectEnabled(node.name.clone()));
        }
    }
    let cap = ports_for(graph, capture, Direction::Input, &mut violations);
    let play = ports_for(graph, playback, Direction::Output, &mut violations);
    let target = ports_for(graph, sink, Direction::Input, &mut violations);
    let mpd_ports = ports_for(graph, mpd, Direction::Output, &mut violations);
    if !violations.is_empty() {
        return RouteStatus::Invalid(violations);
    }
    let (Some(cap), Some(play), Some(target)) = (cap, play, target) else {
        return RouteStatus::NotReady;
    };
    let mut expected = HashSet::new();
    for index in 0..2 {
        expected.insert((play[index].id, target[index].id));
        if let Some(mpd_ports) = mpd_ports {
            expected.insert((mpd_ports[index].id, cap[index].id));
        }
    }
    let owned_outputs: HashSet<u32> = play
        .iter()
        .map(|p| p.id)
        .chain(mpd_ports.iter().flatten().map(|p| p.id))
        .collect();
    let owned_inputs: HashSet<u32> = cap.iter().map(|p| p.id).collect();
    let mut observed = HashSet::new();
    for link in &graph.links {
        if owned_outputs.contains(&link.output_port) || owned_inputs.contains(&link.input_port) {
            let pair = (link.output_port, link.input_port);
            if !expected.contains(&pair) {
                violations.push(RouteViolation::ExtraLink(link.id));
            } else if !observed.insert(pair) {
                violations.push(RouteViolation::DuplicateLink(pair.0, pair.1));
            } else if !link.active {
                violations.push(RouteViolation::InactiveLink(link.id));
            }
        }
    }
    for (out, input) in expected.difference(&observed) {
        violations.push(RouteViolation::MissingLink(*out, *input));
    }
    if !violations.is_empty() {
        RouteStatus::Invalid(violations)
    } else if require_mpd {
        RouteStatus::RouteReady
    } else {
        RouteStatus::CamillaReady
    }
}

pub trait PipeWireCommandRunner {
    fn run(
        &mut self,
        program: &'static str,
        operation: &'static str,
        args: &[String],
    ) -> Result<Vec<u8>, DspError>;
}

pub struct SystemCommandRunner;

fn safe_arg(arg: &str) -> Result<(), DspError> {
    if arg.contains(['\0', '\n', '\r']) || arg.is_empty() {
        Err(DspError::InvalidIdentifier)
    } else {
        Ok(())
    }
}

fn endpoint(node: &Node, port: &Port) -> Result<String, DspError> {
    if port.node_id != node.id || node.name.contains(':') || port.name.contains(':') {
        return Err(DspError::InvalidIdentifier);
    }
    safe_arg(&node.name)?;
    safe_arg(&port.name)?;
    Ok(format!("{}:{}", node.name, port.name))
}

impl PipeWireCommandRunner for SystemCommandRunner {
    fn run(
        &mut self,
        program: &'static str,
        operation: &'static str,
        args: &[String],
    ) -> Result<Vec<u8>, DspError> {
        for arg in args {
            safe_arg(arg)?;
        }
        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    DspError::PipeWireToolMissing(program)
                } else {
                    DspError::CommandFailed {
                        program,
                        operation,
                        status: None,
                        stderr: e.to_string(),
                    }
                }
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or(DspError::SnapshotFailed("missing stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(DspError::SnapshotFailed("missing stderr pipe".into()))?;
        let (status, output, diagnostics) = thread::scope(|scope| {
            use std::io::Read;
            let output_reader = scope.spawn(move || {
                let mut bytes = Vec::new();
                let _ = stdout.take(32 * 1024 * 1024).read_to_end(&mut bytes);
                bytes
            });
            let error_reader = scope.spawn(move || {
                let mut bytes = Vec::new();
                let _ = stderr.take(64 * 1024).read_to_end(&mut bytes);
                bytes
            });
            let deadline = Instant::now() + Duration::from_secs(8);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25))
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(DspError::CommandFailed {
                            program,
                            operation,
                            status: None,
                            stderr: "timed out".into(),
                        });
                    }
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(DspError::CommandFailed {
                            program,
                            operation,
                            status: None,
                            stderr: "process status failed".into(),
                        });
                    }
                }
            };
            (
                status,
                output_reader.join().unwrap_or_default(),
                error_reader.join().unwrap_or_default(),
            )
        });
        let status = status?;
        if !status.success() {
            return Err(DspError::CommandFailed {
                program,
                operation,
                status: status.code(),
                stderr: sanitize_stderr(&diagnostics),
            });
        }
        Ok(output)
    }
}

fn sanitize_stderr(stderr: &[u8]) -> String {
    // Avoid echoing untrusted node names, URIs or tokens from external tools.
    if stderr.is_empty() {
        "no stderr".into()
    } else {
        "see PipeWire tool diagnostics".into()
    }
}

pub struct PipeWireRouteManager<R: PipeWireCommandRunner> {
    runner: R,
    owned_links: Vec<u32>,
}

impl<R: PipeWireCommandRunner> PipeWireRouteManager<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            owned_links: Vec::new(),
        }
    }

    pub fn snapshot(&mut self) -> Result<Graph, DspError> {
        parse_snapshot(&self.runner.run("pw-dump", "snapshot", &[])?)
    }

    pub fn link_stereo(
        &mut self,
        source_node: &Node,
        source: [&Port; 2],
        destination_node: &Node,
        destination: [&Port; 2],
    ) -> Result<(), DspError> {
        for index in 0..2 {
            if source[index].direction != Direction::Output
                || destination[index].direction != Direction::Input
                || source[index].channel != destination[index].channel
                || source[index].channel != ["FL", "FR"][index]
            {
                return Err(DspError::PortMissing(
                    "stereo channel/direction mismatch".into(),
                ));
            }
        }
        for index in 0..2 {
            let args = [
                endpoint(source_node, source[index])?,
                endpoint(destination_node, destination[index])?,
            ];
            self.runner
                .run("pw-link", "create link", &args)
                .map_err(|_| DspError::LinkCreateFailed)?;
            let graph = match self.snapshot() {
                Ok(graph) => graph,
                Err(error) => {
                    let _ = self.runner.run(
                        "pw-link",
                        "remove unverified link",
                        &["-d".into(), args[0].clone(), args[1].clone()],
                    );
                    return Err(error);
                }
            };
            let matches: Vec<_> = graph
                .links
                .iter()
                .filter(|link| {
                    link.output_port == source[index].id && link.input_port == destination[index].id
                })
                .collect();
            if matches.len() != 1 {
                let _ = self.runner.run(
                    "pw-link",
                    "remove unverified link",
                    &["-d".into(), args[0].clone(), args[1].clone()],
                );
                return Err(DspError::LinkCreateFailed);
            }
            self.owned_links.push(matches[0].id);
        }
        Ok(())
    }

    pub fn remove_owned_links(&mut self) -> Result<(), DspError> {
        let mut errors = false;
        for id in std::mem::take(&mut self.owned_links).into_iter().rev() {
            let args = ["-d".into(), id.to_string()];
            if self.runner.run("pw-link", "remove link", &args).is_err() {
                self.owned_links.push(id);
                errors = true;
            }
        }
        if errors {
            Err(DspError::LinkRemoveFailed)
        } else {
            Ok(())
        }
    }

    pub fn owned_links(&self) -> &[u32] {
        &self.owned_links
    }
}

pub struct PipeWireMonitor {
    child: Child,
    reader: Option<JoinHandle<()>>,
    receiver: Receiver<()>,
    pub session: u64,
}

impl PipeWireMonitor {
    pub fn start(session: u64) -> Result<Self, DspError> {
        let mut child = Command::new("pw-link")
            .arg("-m")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DspError::MonitorFailed)?;
        let stdout = child.stdout.take().ok_or(DspError::MonitorFailed)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if !line.is_empty() => match sender.try_send(()) {
                        Ok(()) | Err(TrySendError::Full(())) => {}
                        Err(TrySendError::Disconnected(())) => break,
                    },
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            child,
            reader: Some(reader),
            receiver,
            session,
        })
    }

    pub fn event_pending(&mut self) -> Result<bool, DspError> {
        if self
            .child
            .try_wait()
            .map_err(|_| DspError::MonitorFailed)?
            .is_some()
        {
            return Err(DspError::MonitorFailed);
        }
        Ok(self.receiver.try_recv().is_ok())
    }

    pub fn stop(mut self) -> Result<(), DspError> {
        if self
            .child
            .try_wait()
            .map_err(|_| DspError::MonitorFailed)?
            .is_none()
        {
            self.child.kill().map_err(|_| DspError::MonitorFailed)?;
        }
        self.child.wait().map_err(|_| DspError::MonitorFailed)?;
        if let Some(reader) = self.reader.take() {
            reader.join().map_err(|_| DspError::MonitorFailed)?;
        }
        Ok(())
    }
}

impl Drop for PipeWireMonitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph() -> (Graph, DspRoute) {
        let nodes = [
            (1, "mpd", "Stream/Output/Audio"),
            (2, "capture", "Stream/Input/Audio"),
            (3, "playback", "Stream/Output/Audio"),
            (4, "sonante_dsp_null", "Audio/Sink"),
        ]
        .into_iter()
        .map(|(id, name, media_class)| Node {
            id,
            serial: Some(id as u64),
            name: name.into(),
            media_class: media_class.into(),
            autoconnect: if id == 4 { None } else { Some(false) },
            virtual_sink: id == 4,
        })
        .collect();
        let mut ports = Vec::new();
        for (node_id, direction) in [
            (1, Direction::Output),
            (2, Direction::Input),
            (3, Direction::Output),
            (4, Direction::Input),
        ] {
            for (index, channel) in ["FL", "FR"].into_iter().enumerate() {
                ports.push(Port {
                    id: node_id * 10 + index as u32,
                    node_id,
                    name: format!(
                        "{}_{channel}",
                        if node_id == 4 {
                            "playback"
                        } else if direction == Direction::Input {
                            "input"
                        } else {
                            "output"
                        }
                    ),
                    channel: channel.into(),
                    direction,
                });
            }
        }
        let links = [(10, 20), (11, 21), (30, 40), (31, 41)]
            .into_iter()
            .enumerate()
            .map(|(id, (output_port, input_port))| Link {
                id: 100 + id as u32,
                output_port,
                input_port,
                active: true,
            })
            .collect();
        (
            Graph {
                nodes,
                ports,
                links,
            },
            DspRoute {
                mpd_name: Some("mpd".into()),
                capture_name: "capture".into(),
                playback_name: "playback".into(),
                sink_name: "sonante_dsp_null".into(),
            },
        )
    }

    #[test]
    fn valid_route_and_camilla_only() {
        let (mut graph, route) = graph();
        assert_eq!(
            validate_topology(&graph, &route, true),
            RouteStatus::RouteReady
        );
        graph.links.retain(|l| l.output_port >= 30);
        assert!(
            matches!(validate_topology(&graph, &route, false), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::MissingLink(10, 20)) && v.contains(&RouteViolation::MissingLink(11, 21)))
        );
        let mut camilla_route = route.clone();
        camilla_route.mpd_name = None;
        assert_eq!(
            validate_topology(&graph, &camilla_route, false),
            RouteStatus::CamillaReady
        );
    }

    #[test]
    fn missing_link_and_disappearing_nodes() {
        let (mut graph, route) = graph();
        graph.links.pop();
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::MissingLink(31, 41)))
        );
        graph.nodes.retain(|n| n.id != 4);
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::MissingNode("sonante_dsp_null".into())))
        );
        graph.nodes.retain(|n| n.id != 2);
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::MissingNode("capture".into())))
        );
    }

    #[test]
    fn forbidden_direct_wrong_sink_and_swapped_channel() {
        let (mut graph, route) = graph();
        graph.links.push(Link {
            id: 105,
            output_port: 10,
            input_port: 40,
            active: true,
        });
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::ExtraLink(105)))
        );
        graph.links.pop();
        graph.links[2].input_port = 99;
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::ExtraLink(102)))
        );
        graph.links[2].input_port = 41;
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::ExtraLink(102)))
        );
    }

    #[test]
    fn duplicate_nodes_and_links_are_invalid() {
        let (mut graph, route) = graph();
        graph.nodes.push(graph.nodes[1].clone());
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::DuplicateNode("capture".into())))
        );
        graph.nodes.pop();
        graph.links.push(graph.links[0].clone());
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::DuplicateLink(10, 20)))
        );
    }

    #[test]
    fn inactive_link_never_validates_route() {
        let (mut graph, route) = graph();
        graph.links[0].active = false;
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::InactiveLink(100)))
        );
    }

    #[test]
    fn missing_mpd_is_not_ready_and_bad_ports_fail() {
        let (mut graph, route) = graph();
        graph.nodes.retain(|n| n.id != 1);
        assert_eq!(
            validate_topology(&graph, &route, true),
            RouteStatus::NotReady
        );
        graph.nodes.push(Node {
            id: 1,
            serial: None,
            name: "mpd".into(),
            media_class: "Stream/Output/Audio".into(),
            autoconnect: Some(false),
            virtual_sink: false,
        });
        graph.ports[0].channel = "FR".into();
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::PortInvalid("mpd".into())))
        );
        graph.ports[0].channel = "FL".into();
        graph.ports[0].direction = Direction::Input;
        assert!(
            matches!(validate_topology(&graph, &route, true), RouteStatus::Invalid(v)
            if v.contains(&RouteViolation::PortInvalid("mpd".into())))
        );
    }

    #[test]
    fn node_discovery_zero_one_duplicate_and_class() {
        let (mut graph, _) = graph();
        assert_eq!(
            exact_node(&graph, "missing", "Audio/Sink"),
            Err(DspError::NodeMissing("missing".into()))
        );
        assert_eq!(
            exact_node(&graph, "mpd", "Stream/Output/Audio").map(|n| n.id),
            Ok(1)
        );
        assert!(matches!(
            exact_node(&graph, "mpd", "Audio/Sink"),
            Err(DspError::TopologyInvalid(_))
        ));
        graph.nodes.push(graph.nodes[0].clone());
        assert_eq!(
            exact_node(&graph, "mpd", "Stream/Output/Audio"),
            Err(DspError::NodeAmbiguous("mpd".into()))
        );
    }

    #[test]
    fn parser_uses_port_and_link_ids_from_pw_dump() {
        let raw = json!([
            {"id":1,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"mpd","media.class":"Stream/Output/Audio","object.serial":"77","node.autoconnect":false}}},
            {"id":2,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"null","media.class":"Audio/Sink","factory.name":"support.null-audio-sink"}}},
            {"id":10,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":"1","port.name":"output_FL","port.direction":"out","audio.channel":"FL"}}},
            {"id":100,"type":"PipeWire:Interface:Link","info":{"state":"active","props":{"link.output.port":"10","link.input.port":"20"}}}
        ]);
        let graph = parse_snapshot(raw.to_string().as_bytes()).unwrap();
        assert_eq!(graph.nodes[0].serial, Some(77));
        assert!(!graph.nodes[0].virtual_sink);
        assert!(graph.nodes[1].virtual_sink);
        assert_eq!(graph.ports[0].direction, Direction::Output);
        assert_eq!(graph.links[0].output_port, 10);
        assert!(graph.links[0].active);
        assert!(parse_snapshot(b"not json").is_err());
    }

    #[test]
    fn arguments_reject_control_characters_without_shell() {
        for value in ["a\nb", "a\rb", "a\0b", ""] {
            assert_eq!(safe_arg(value), Err(DspError::InvalidIdentifier));
        }
        assert!(safe_arg("node with spaces ' 🎵").is_ok());
        let (graph, _) = graph();
        let mut unsafe_node = graph.nodes[0].clone();
        unsafe_node.name = "other:sink".into();
        assert_eq!(
            endpoint(&unsafe_node, &graph.ports[0]),
            Err(DspError::InvalidIdentifier)
        );
    }

    #[test]
    fn missing_pipewire_tool_is_structured() {
        let mut runner = SystemCommandRunner;
        assert_eq!(
            runner.run("sonante-nonexistent-pw-tool", "probe", &[]),
            Err(DspError::PipeWireToolMissing("sonante-nonexistent-pw-tool"))
        );
    }

    struct FakeRunner {
        graph: Graph,
        calls: Vec<(String, Vec<String>)>,
        fail_remove: bool,
    }
    impl PipeWireCommandRunner for FakeRunner {
        fn run(
            &mut self,
            program: &'static str,
            _: &'static str,
            args: &[String],
        ) -> Result<Vec<u8>, DspError> {
            self.calls.push((program.into(), args.to_vec()));
            if program == "pw-dump" {
                let objects: Vec<_> = self
                    .graph
                    .links
                    .iter()
                    .map(|l| {
                        json!({"id":l.id,
                    "type":"PipeWire:Interface:Link","info":{"state":if l.active {"active"} else {"error"},"props":{
                        "link.output.port":l.output_port,"link.input.port":l.input_port}}})
                    })
                    .collect();
                return Ok(serde_json::to_vec(&objects).unwrap());
            }
            if args.first().is_some_and(|s| s == "-d") {
                if self.fail_remove {
                    return Err(DspError::LinkRemoveFailed);
                }
                let id: u32 = args[1].parse().unwrap();
                self.graph.links.retain(|l| l.id != id);
            } else {
                let port_id = |endpoint: &str| -> u32 {
                    let (node_name, port_name) = endpoint.split_once(':').unwrap();
                    let node = self
                        .graph
                        .nodes
                        .iter()
                        .find(|n| n.name == node_name)
                        .unwrap();
                    self.graph
                        .ports
                        .iter()
                        .find(|p| p.node_id == node.id && p.name == port_name)
                        .unwrap()
                        .id
                };
                let out = port_id(&args[0]);
                let input = port_id(&args[1]);
                self.graph.links.push(Link {
                    id: 200 + self.graph.links.len() as u32,
                    output_port: out,
                    input_port: input,
                    active: true,
                });
            }
            Ok(Vec::new())
        }
    }

    #[test]
    fn owned_links_only_are_removed_in_reverse_order() {
        let (mut graph, _) = graph();
        graph.links.clear();
        graph.links.push(Link {
            id: 1,
            output_port: 90,
            input_port: 91,
            active: true,
        });
        let ports = graph.ports.clone();
        let nodes = graph.nodes.clone();
        let fake = FakeRunner {
            graph,
            calls: Vec::new(),
            fail_remove: false,
        };
        let mut manager = PipeWireRouteManager::new(fake);
        manager
            .link_stereo(
                &nodes[2],
                [&ports[4], &ports[5]],
                &nodes[3],
                [&ports[6], &ports[7]],
            )
            .unwrap();
        assert_eq!(manager.owned_links(), &[201, 202]);
        assert_eq!(
            manager.runner.calls[0].1,
            vec!["playback:output_FL", "sonante_dsp_null:playback_FL"]
        );
        manager.remove_owned_links().unwrap();
        assert_eq!(manager.runner.graph.links.len(), 1);
        assert_eq!(manager.runner.calls.last().unwrap().1, vec!["-d", "201"]);
    }

    #[test]
    fn failed_link_removal_retains_ownership_for_retry() {
        let (mut graph, _) = graph();
        graph.links.clear();
        let ports = graph.ports.clone();
        let nodes = graph.nodes.clone();
        let fake = FakeRunner {
            graph,
            calls: Vec::new(),
            fail_remove: false,
        };
        let mut manager = PipeWireRouteManager::new(fake);
        manager
            .link_stereo(
                &nodes[2],
                [&ports[4], &ports[5]],
                &nodes[3],
                [&ports[6], &ports[7]],
            )
            .unwrap();
        manager.runner.fail_remove = true;
        assert_eq!(
            manager.remove_owned_links(),
            Err(DspError::LinkRemoveFailed)
        );
        assert_eq!(manager.owned_links().len(), 2);
    }
}
