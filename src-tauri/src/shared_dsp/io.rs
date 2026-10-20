//! PipeWire observation and links for this owned Camilla playback stream only.
use super::Error;
use serde_json::Value;
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Default)]
pub(crate) struct Environment {
    pub vars: Vec<(String, String)>,
}
impl Environment {
    pub fn command(&self, program: &str) -> Command {
        let fixed;
        let program = if !cfg!(debug_assertions) && matches!(program, "pw-dump" | "pw-link" | "pw-cli") {
            fixed = format!("/usr/bin/{program}");
            fixed.as_str()
        } else { program };
        let mut c = Command::new(program);
        for (k, v) in &self.vars {
            c.env(k, v);
        }
        c
    }
    pub fn run(&self, program: &str, args: &[&str]) -> Result<Vec<u8>, Error> {
        let mut child = self
            .command(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::PipeWireUnavailable)?;
        let stdout = child.stdout.take().ok_or(Error::PipeWireUnavailable)?;
        let reader = thread::spawn(move || {
            let mut b = Vec::new();
            stdout.take(16 * 1024 * 1024 + 1).read_to_end(&mut b).ok();
            b
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let bytes = reader.join().map_err(|_| Error::PipeWireUnavailable)?;
        if bytes.len() > 16 * 1024 * 1024 || !status.is_some_and(|s| s.success()) {
            return Err(Error::PipeWireUnavailable);
        }
        Ok(bytes)
    }
    pub fn graph(&self) -> Result<Graph, Error> {
        Graph::parse(&self.run("pw-dump", &[])?)
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub id: u32,
    pub serial: u64,
    pub name: String,
    pub class: String,
    pub pid: Option<u32>,
}
#[derive(Clone)]
pub(crate) struct Port {
    pub id: u32,
    pub node: u32,
    pub name: String,
    pub channel: String,
    pub output: bool,
}
#[derive(Clone)]
struct Link {
    out: u32,
    input: u32,
    active: bool,
}
pub(crate) struct Graph {
    pub nodes: Vec<Node>,
    pub ports: Vec<Port>,
    links: Vec<Link>,
    default: Option<String>,
}
fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
impl Graph {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let objects: Vec<Value> = serde_json::from_slice(bytes).map_err(|_| Error::GraphInvalid)?;
        if objects.len() > 100_000 {
            return Err(Error::GraphInvalid);
        }
        let clients = objects
            .iter()
            .filter(|v| v["type"] == "PipeWire:Interface:Client")
            .filter_map(|v| {
                Some((
                    number(&v["id"])? as u32,
                    number(&v["info"]["props"]["application.process.id"])? as u32,
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut g = Self {
            nodes: vec![],
            ports: vec![],
            links: vec![],
            default: None,
        };
        for v in objects {
            let Some(id) = number(&v["id"]).and_then(|i| u32::try_from(i).ok()) else {
                continue;
            };
            let p = &v["info"]["props"];
            match v["type"].as_str() {
                Some("PipeWire:Interface:Node") => {
                    if let (Some(name), Some(class), Some(serial)) = (
                        p["node.name"].as_str(),
                        p["media.class"].as_str(),
                        number(&p["object.serial"]),
                    ) {
                        let pid = number(&p["application.process.id"])
                            .map(|n| n as u32)
                            .or_else(|| {
                                number(&p["client.id"])
                                    .and_then(|n| clients.get(&(n as u32)).copied())
                            });
                        g.nodes.push(Node {
                            id,
                            serial,
                            name: name.into(),
                            class: class.into(),
                            pid,
                        });
                    }
                }
                Some("PipeWire:Interface:Port") => {
                    if let (Some(node), Some(name), Some(channel), Some(dir)) = (
                        number(&p["node.id"]),
                        p["port.name"].as_str(),
                        p["audio.channel"].as_str(),
                        p["port.direction"].as_str(),
                    ) {
                        g.ports.push(Port {
                            id,
                            node: node as u32,
                            name: name.into(),
                            channel: channel.into(),
                            output: dir == "out",
                        });
                    }
                }
                Some("PipeWire:Interface:Link") => {
                    if let (Some(out), Some(input)) = (
                        number(&v["info"]["output-port-id"]),
                        number(&v["info"]["input-port-id"]),
                    ) {
                        g.links.push(Link {
                            out: out as u32,
                            input: input as u32,
                            active: v["info"]["state"] == "active",
                        });
                    }
                }
                Some("PipeWire:Interface:Metadata") => {
                    if let Some(items) = v["metadata"]
                        .as_array()
                        .or_else(|| v["info"]["metadata"].as_array())
                    {
                        for item in items {
                            if item["key"] == "default.audio.sink" {
                                let value = if let Some(s) = item["value"].as_str() {
                                    serde_json::from_str(s).unwrap_or(Value::Null)
                                } else {
                                    item["value"].clone()
                                };
                                if let Some(name) = value["name"].as_str() {
                                    g.default = Some(name.into());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(g)
    }
    pub fn node(&self, name: &str, class: &str) -> Result<&Node, Error> {
        let mut nodes = self
            .nodes
            .iter()
            .filter(|n| n.name == name && n.class == class);
        let n = nodes.next().ok_or(Error::NodeMissing)?;
        if nodes.next().is_some() {
            return Err(Error::GraphInvalid);
        }
        Ok(n)
    }
    pub fn default_sink(&self) -> Result<Node, Error> {
        if let Some(name) = &self.default {
            return Ok(self.node(name, "Audio/Sink")?.clone());
        }
        let mut sinks = self.nodes.iter().filter(|n| n.class == "Audio/Sink");
        let n = sinks.next().ok_or(Error::SinkUnavailable)?;
        if sinks.next().is_some() {
            return Err(Error::SinkUnavailable);
        }
        Ok(n.clone())
    }
    pub fn stereo(&self, node: &Node, output: bool) -> Result<[Port; 2], Error> {
        let get = |channel: &str| {
            let mut p = self
                .ports
                .iter()
                .filter(|p| p.node == node.id && p.output == output && p.channel == channel);
            let one = p.next().ok_or(Error::GraphInvalid)?;
            if p.next().is_some() {
                return Err(Error::GraphInvalid);
            }
            Ok(one.clone())
        };
        Ok([get("FL")?, get("FR")?])
    }
    pub fn validate(&self, stream: &Node, sink: &Node) -> Result<bool, Error> {
        if self.default.as_ref().is_some_and(|name| name != &sink.name) {
            return Err(Error::GraphInvalid);
        }
        let actual = self.node(&stream.name, "Stream/Output/Audio")?;
        let dest = self.node(&sink.name, "Audio/Sink")?;
        if actual.serial != stream.serial || actual.pid != stream.pid || dest.serial != sink.serial
        {
            return Err(Error::GraphInvalid);
        }
        let outputs = self.stereo(actual, true)?;
        let inputs = self.stereo(dest, false)?;
        let mut active = true;
        for i in 0..2 {
            let links = self
                .links
                .iter()
                .filter(|l| l.out == outputs[i].id)
                .collect::<Vec<_>>();
            if links.len() != 1 || links[0].input != inputs[i].id {
                return Err(Error::GraphInvalid);
            }
            active &= links[0].active;
        }
        Ok(active)
    }
    pub fn connect(&self, env: &Environment, stream: &Node, sink: &Node) -> Result<(), Error> {
        let outputs = self.stereo(stream, true)?;
        let inputs = self.stereo(sink, false)?;
        if self
            .links
            .iter()
            .any(|l| outputs.iter().any(|p| p.id == l.out))
        {
            return Err(Error::GraphInvalid);
        }
        for i in 0..2 {
            let out = endpoint(&stream.name, &outputs[i].name)?;
            let input = endpoint(&sink.name, &inputs[i].name)?;
            env.run("pw-link", &[&out, &input])?;
        }
        Ok(())
    }
}
fn endpoint(node: &str, port: &str) -> Result<String, Error> {
    if [node, port]
        .iter()
        .any(|s| s.is_empty() || s.contains(['\0', '\n', '\r', ':']))
    {
        return Err(Error::GraphInvalid);
    }
    Ok(format!("{node}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Graph {
        Graph {
            nodes: vec![
                Node {
                    id: 1,
                    serial: 11,
                    name: "owned".into(),
                    class: "Stream/Output/Audio".into(),
                    pid: Some(100),
                },
                Node {
                    id: 2,
                    serial: 22,
                    name: "sink".into(),
                    class: "Audio/Sink".into(),
                    pid: None,
                },
            ],
            ports: [
                (3, 1, "FL", true),
                (4, 1, "FR", true),
                (5, 2, "FL", false),
                (6, 2, "FR", false),
            ]
            .into_iter()
            .map(|(id, node, c, output)| Port {
                id,
                node,
                name: c.into(),
                channel: c.into(),
                output,
            })
            .collect(),
            links: vec![
                Link {
                    out: 3,
                    input: 5,
                    active: true,
                },
                Link {
                    out: 4,
                    input: 6,
                    active: true,
                },
            ],
            default: Some("sink".into()),
        }
    }
    #[test]
    fn route_requires_exact_owned_process_serial_channels_single_links_and_sink() {
        let mut g = fixture();
        let stream = g.nodes[0].clone();
        let sink = g.nodes[1].clone();
        assert_eq!(g.validate(&stream, &sink), Ok(true));
        g.links[0].active = false;
        assert_eq!(g.validate(&stream, &sink), Ok(false));
        g.links.push(Link {
            out: 3,
            input: 6,
            active: true,
        });
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
        g.links.pop();
        g.nodes[0].pid = Some(999);
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
        g.nodes[0] = stream.clone();
        g.default = Some("other".into());
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
        g.default = Some("sink".into());
        g.nodes[1].serial += 1;
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
    }
    #[test]
    fn node_ambiguity_and_unknown_channel_are_not_assumed_stereo() {
        let mut g = fixture();
        let s = g.nodes[0].clone();
        g.ports[0].channel = "MONO".into();
        assert!(g.stereo(&s, true).is_err());
        g.nodes.push(s);
        assert!(g.node("owned", "Stream/Output/Audio").is_err());
        assert!(endpoint("sink:foreign", "FL").is_err());
    }
}
