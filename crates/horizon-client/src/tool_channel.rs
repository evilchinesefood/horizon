//! Bounded native tool controls and transient, server-authored building previews.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

pub const CHANNEL: &str = "horizon:tools";
pub const MAX_BYTES: usize = 128 * 1024;
pub const PREVIEW_TTL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlAdvert {
    pub name: String,
    pub label: String,
    pub key: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolAdvert {
    pub module: String,
    pub generation: u64,
    pub item: String,
    pub controls: Vec<ControlAdvert>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMessage {
    Registry {
        tools: Vec<ToolAdvert>,
    },
    Preview {
        module: String,
        generation: u64,
        item: String,
        dimension: String,
        positions: Vec<[i32; 3]>,
        label: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Control {
        module: String,
        generation: u64,
        item: String,
        action: String,
        sequence: u64,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    TooLarge,
    InvalidJson,
    InvalidMessage,
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProtocolError {}
fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
}
fn id(s: &str) -> bool {
    if s.len() > 256 {
        return false;
    }
    let Some((namespace, path)) = s.split_once(':') else {
        return false;
    };
    token(namespace)
        && !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
}
fn key(s: &str) -> bool {
    s.len() <= 128
        && s.strip_prefix("key.keyboard.").is_some_and(|tail| {
            !tail.is_empty()
                && tail
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.')
        })
}
fn label(s: &str) -> bool {
    s.len() <= 256 && !s.chars().any(char::is_control)
}
fn identity(module: &str, generation: u64, item: &str) -> bool {
    token(module) && generation > 0 && id(item)
}
impl ServerMessage {
    fn valid(&self) -> bool {
        match self {
            Self::Registry { tools } => {
                let mut items = BTreeSet::new();
                let mut generations = BTreeMap::new();
                tools.len() <= 256
                    && tools.iter().all(|t| {
                        let mut names = BTreeSet::new();
                        let mut keys = BTreeSet::new();
                        identity(&t.module, t.generation, &t.item)
                            && generations
                                .insert(&t.module, t.generation)
                                .is_none_or(|g| g == t.generation)
                            && items.insert(&t.item)
                            && t.controls.len() <= 16
                            && t.controls.iter().all(|c| {
                                token(&c.name)
                                    && label(&c.label)
                                    && key(&c.key)
                                    && names.insert(&c.name)
                                    && keys.insert(&c.key)
                            })
                    })
            }
            Self::Preview {
                module,
                generation,
                item,
                dimension,
                positions,
                label: text,
            } => {
                let mut seen = BTreeSet::new();
                identity(module, *generation, item)
                    && id(dimension)
                    && label(text)
                    && positions.len() <= 4096
                    && positions.iter().all(|p| {
                        p.iter().all(|v| (-30_000_000..=30_000_000).contains(v)) && seen.insert(*p)
                    })
            }
        }
    }
}
impl ClientMessage {
    fn valid(&self) -> bool {
        match self {
            Self::Control {
                module,
                generation,
                item,
                action,
                sequence,
            } => identity(module, *generation, item) && token(action) && *sequence > 0,
        }
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolError> {
    if bytes.len() > MAX_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| ProtocolError::InvalidJson)
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ProtocolError::InvalidJson)?;
    if bytes.len() > MAX_BYTES {
        Err(ProtocolError::TooLarge)
    } else {
        Ok(bytes)
    }
}
pub fn decode_server(bytes: &[u8]) -> Result<ServerMessage, ProtocolError> {
    let value: ServerMessage = decode(bytes)?;
    if value.valid() {
        Ok(value)
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}
pub fn decode_client(bytes: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let value: ClientMessage = decode(bytes)?;
    if value.valid() {
        Ok(value)
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}
pub fn encode_server(value: &ServerMessage) -> Result<Vec<u8>, ProtocolError> {
    if value.valid() {
        encode(value)
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}
pub fn encode_client(value: &ClientMessage) -> Result<Vec<u8>, ProtocolError> {
    if value.valid() {
        encode(value)
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}

#[derive(Default)]
pub struct ToolClient {
    tools: Vec<ToolAdvert>,
    preview: Option<(ServerMessage, Instant)>,
    context: Option<(String, String)>,
    sequence: u64,
}
impl ToolClient {
    /// Registry ownership only; callers supply the actual held item.
    pub fn claims_item(&self, item: &str) -> bool {
        self.tools.iter().any(|tool| tool.item == item)
    }

    /// World/server transitions must discard all advertised ownership and previews.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    /// Call even while hidden, so a preview cannot reappear after switching away.
    pub fn observe(&mut self, held: Option<&str>, dimension: &str, visible: bool) {
        let context = held
            .filter(|_| visible)
            .map(|item| (item.to_owned(), dimension.to_owned()));
        if self.context != context {
            self.preview = None;
            self.context = context;
        }
    }
    pub fn receive(&mut self, message: ServerMessage, now: Instant) -> bool {
        if !message.valid() {
            return false;
        }
        match &message {
            ServerMessage::Registry { tools } => {
                if tools.iter().any(|new| {
                    self.tools
                        .iter()
                        .any(|old| new.module == old.module && new.generation < old.generation)
                }) {
                    return false;
                }
                self.tools = tools.clone();
                self.preview = None;
                true
            }
            ServerMessage::Preview {
                module,
                generation,
                item,
                dimension,
                ..
            } => {
                if self
                    .context
                    .as_ref()
                    .is_none_or(|(held, dim)| held != item || dim != dimension)
                    || !self.tools.iter().any(|t| {
                        t.module == *module && t.generation == *generation && t.item == *item
                    })
                {
                    return false;
                }
                self.preview = Some((message, now));
                true
            }
        }
    }
    pub fn control(&mut self, key: &str) -> Option<ClientMessage> {
        let (item, _) = self.context.as_ref()?;
        let tool = self.tools.iter().find(|t| t.item == *item)?;
        let action = &tool.controls.iter().find(|c| c.key == key)?.name;
        self.sequence = self.sequence.checked_add(1)?;
        Some(ClientMessage::Control {
            module: tool.module.clone(),
            generation: tool.generation,
            item: item.clone(),
            action: action.clone(),
            sequence: self.sequence,
        })
    }
    pub fn preview(&self, now: Instant) -> Option<&ServerMessage> {
        let (message, received) = self.preview.as_ref()?;
        (now.checked_duration_since(*received)? < PREVIEW_TTL).then_some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registry(generation: u64) -> ServerMessage {
        ServerMessage::Registry {
            tools: vec![ToolAdvert {
                module: "builder".into(),
                generation,
                item: "builder:wand".into(),
                controls: vec![ControlAdvert {
                    name: "mode".into(),
                    label: "Mode".into(),
                    key: "key.keyboard.m".into(),
                }],
            }],
        }
    }
    fn preview(generation: u64) -> ServerMessage {
        ServerMessage::Preview {
            module: "builder".into(),
            generation,
            item: "builder:wand".into(),
            dimension: "minecraft:overworld".into(),
            positions: vec![[0, 64, 0]],
            label: "1 block".into(),
        }
    }
    #[test]
    fn codecs_and_bounds() {
        let r = registry(1);
        assert_eq!(decode_server(&encode_server(&r).unwrap()).unwrap(), r);
        let mut client = ToolClient::default();
        client.observe(Some("builder:wand"), "minecraft:overworld", true);
        client.receive(r, Instant::now());
        let c = client.control("key.keyboard.m").unwrap();
        assert_eq!(decode_client(&encode_client(&c).unwrap()).unwrap(), c);
        assert_eq!(
            decode_server(&vec![b' '; MAX_BYTES + 1]),
            Err(ProtocolError::TooLarge)
        );
        assert!(decode_server(br#"{"type":"registry","tools":[],"extra":1}"#).is_err());
        let mut r = registry(1);
        if let ServerMessage::Registry { tools } = &mut r {
            tools.push(tools[0].clone());
        }
        assert!(encode_server(&r).is_err());
        let mut p = preview(1);
        if let ServerMessage::Preview { positions, .. } = &mut p {
            *positions = vec![[0, 0, 0]; 4097];
        }
        assert!(encode_server(&p).is_err());
        let mut p = preview(1);
        if let ServerMessage::Preview { item, .. } = &mut p {
            *item = "INVALID".into();
        }
        assert!(encode_server(&p).is_err());
    }
    #[test]
    fn registry_control_and_coordinate_limits() {
        let mut r = registry(1);
        if let ServerMessage::Registry { tools } = &mut r {
            tools[0].controls = vec![tools[0].controls[0].clone(); 17];
        }
        assert!(encode_server(&r).is_err());
        let mut r = registry(1);
        if let ServerMessage::Registry { tools } = &mut r {
            tools[0].controls[0].key = "not.a.keyboard.binding".into();
        }
        assert!(encode_server(&r).is_err());
        let mut p = preview(1);
        if let ServerMessage::Preview { positions, .. } = &mut p {
            positions[0][0] = i32::MIN;
        }
        assert!(encode_server(&p).is_err());
        let now = Instant::now();
        let mut c = ToolClient::default();
        c.observe(Some("builder:wand"), "minecraft:overworld", true);
        assert!(c.receive(registry(2), now));
        assert!(c.claims_item("builder:wand"));
        assert!(!c.claims_item("minecraft:stick"));
        assert!(!c.receive(registry(1), now));
        assert!(c.receive(preview(2), now));
        c.observe(Some("minecraft:stick"), "minecraft:overworld", true);
        assert!(c.preview(now).is_none());
        assert!(c.control("key.keyboard.m").is_none());
    }

    #[test]
    fn preview_generation_and_lifecycle() {
        let now = Instant::now();
        let mut c = ToolClient::default();
        c.observe(Some("builder:wand"), "minecraft:overworld", true);
        assert!(c.receive(registry(2), now));
        assert!(!c.receive(preview(1), now));
        assert!(c.receive(preview(2), now));
        assert!(c.preview(now).is_some());
        assert!(c.preview(now + PREVIEW_TTL).is_none());
        c.observe(None, "minecraft:overworld", false);
        assert!(c.preview(now).is_none());
        c.observe(Some("builder:wand"), "minecraft:the_nether", true);
        assert!(!c.receive(preview(2), now));
        c.clear();
        assert!(!c.claims_item("builder:wand"));
        assert!(c.control("key.keyboard.m").is_none());
    }
}
