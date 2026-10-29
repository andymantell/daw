// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is who in a shared project (roadmap 1.6, ARCHITECTURE §3), without
//! accounts.
//!
//! Each install keeps a secret and a display name ([`Identity`]). From
//! the secret it works out a signing key for each project, so two
//! projects can't be linked to one person by key
//! (research/PRIVACY_CHILD_DATA.md, implication 4). Members are known by
//! their public key, as hex.
//!
//! The document holds, beside the project itself:
//!
//! - `meta`: the project's `id`, its `owner` (the key of whoever made
//!   it) and `owner_sig`, the owner's signature over both.
//! - `members`: key to a map of `name` (set by that member), `role` and
//!   `grant`, the owner's signature over the role.
//! - `peers`: Loro peer id to `"key signature"`, a member's signature
//!   saying that run of the app was them, so each change in the history
//!   can be put down to a member.
//!
//! Changes merged in from elsewhere are checked
//! ([`ProjectDoc::check`]): the owner may change anything, editors
//! anything but the members and the project's own details, commenters
//! only their own comments, and anyone their own name. Changes from
//! anyone else are refused until the owner lets them in. Signatures
//! prove who a run of the app belongs to, not each change: a live
//! session's link, which is checked against members' keys, covers that.
//! A project with no owner (made before this) isn't checked until its
//! first owner signs in.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use loro::{ContainerID, Frontiers, Index, JsonMapOp, JsonOpContent, LoroDoc, LoroMap, LoroValue};
use sha2::{Digest, Sha256};

use crate::ProjectDoc;

/// Whether each project gets its own key, worked out from the install's
/// secret, rather than one key everywhere.
const KEY_PER_PROJECT: bool = true;

/// Display names are cut to this many characters.
pub const MAX_NAME: usize = 40;

/// The root containers that hold who is who.
const META: &str = "meta";
const MEMBERS: &str = "members";
const PEERS: &str = "peers";
/// Where comments live: the only place commenters may write.
const COMMENTS: &str = "comments";

/// This install: a secret that never leaves the computer, and the name
/// others see.
#[derive(Clone)]
pub struct Identity {
    secret: [u8; 32],
    pub name: String,
}

impl Identity {
    /// A new identity with a fresh secret.
    pub fn generate(name: &str) -> Result<Identity, String> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|e| format!("no random numbers: {e}"))?;
        Ok(Identity {
            secret,
            name: clean_name(name),
        })
    }

    /// Reads what [`to_text`](Self::to_text) wrote.
    pub fn from_text(s: &str) -> Option<Identity> {
        let mut secret = None;
        let mut name = String::new();
        for line in s.lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "secret" => secret = from_hex(v.trim()).and_then(|b| b.try_into().ok()),
                "name" => name = clean_name(v),
                _ => {}
            }
        }
        Some(Identity {
            secret: secret?,
            name,
        })
    }

    /// For keeping in the settings folder. Holds the secret.
    pub fn to_text(&self) -> String {
        format!("secret {}\nname {}\n", hex(&self.secret), self.name)
    }

    /// The key this install signs with in project `project`.
    fn signer(&self, project: &str) -> SigningKey {
        if !KEY_PER_PROJECT {
            return SigningKey::from_bytes(&self.secret);
        }
        let mut h = Sha256::new();
        h.update(b"daw project key\n");
        h.update(self.secret);
        h.update(project.as_bytes());
        SigningKey::from_bytes(&h.finalize().into())
    }

    /// This install's public key in project `project`, as hex.
    pub fn key_for(&self, project: &str) -> String {
        hex(self.signer(project).verifying_key().as_bytes())
    }
}

/// A name as kept: one line, trimmed, not too long.
pub fn clean_name(name: &str) -> String {
    let one_line: String = name.chars().filter(|c| !c.is_control()).collect();
    one_line
        .trim()
        .chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Made the project: may change anything, and says who else may.
    Owner,
    /// May change the project, but not who is in it.
    Editor,
    /// May only add comments, and change or delete their own.
    Commenter,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Editor => "editor",
            Role::Commenter => "commenter",
        }
    }

    fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "editor" => Some(Role::Editor),
            "commenter" => Some(Role::Commenter),
            _ => None,
        }
    }
}

/// Someone named in a project.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// Their public key in this project, as hex.
    pub key: String,
    /// As they gave it; may be empty.
    pub name: String,
    /// `None` for someone who has opened the project but hasn't been let
    /// in.
    pub role: Option<Role>,
}

impl Member {
    /// Their name, or a stand-in made from their key.
    pub fn shown_name(&self) -> String {
        if self.name.is_empty() {
            format!("Someone ({})", &self.key[..self.key.len().min(6)])
        } else {
            self.name.clone()
        }
    }
}

/// What checking another copy's changes found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    /// People whose changes would be refused, as the other copy names
    /// them.
    pub refused: Vec<Member>,
    /// Changes from runs of the app that say nothing about who they
    /// were (an older version of the app, or a damaged file).
    pub unknown: bool,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.refused.is_empty() && !self.unknown
    }

    /// Why the changes weren't merged, in a sentence.
    pub fn reason(&self) -> String {
        let mut names: Vec<String> = self.refused.iter().map(|m| m.shown_name()).collect();
        if self.unknown {
            names.push("an older version of the app".into());
        }
        let who = match names.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        format!("it has changes from {who}, who the project's owner hasn't let in to make them")
    }
}

/// Who this run of the app is, once signed in.
pub(crate) struct Me {
    key: String,
    signer: SigningKey,
}

impl ProjectDoc {
    /// Signs this run of the app in as `me`: gives the project an id and,
    /// if it has no owner yet, makes `me` the owner; records that this run
    /// is `me`, and `me`'s name. Not undoable.
    pub fn sign_in(&mut self, me: &Identity) {
        let meta = self.doc.get_map(META);
        let id = match get_str(&meta, "id") {
            Some(id) => id,
            None => {
                let id = crate::new_id();
                let _ = meta.insert("id", id.as_str());
                id
            }
        };
        let signer = me.signer(&id);
        let key = hex(signer.verifying_key().as_bytes());
        if get_str(&meta, "owner").is_none() {
            // History from before there was an owner is everyone's.
            let since = hex(&self.doc.oplog_frontiers().encode());
            let _ = meta.insert("owner", key.as_str());
            let _ = meta.insert("since", since.as_str());
            let sig = signer.sign(owner_msg(&id, &key, &since).as_bytes());
            let _ = meta.insert("owner_sig", hex(&sig.to_bytes()));
            let m = member_map(&self.doc, &key);
            let _ = m.insert("role", Role::Owner.as_str());
            let sig = signer.sign(grant_msg(&id, &key, Role::Owner).as_bytes());
            let _ = m.insert("grant", hex(&sig.to_bytes()));
        }
        let peer = self.doc.peer_id().to_string();
        let sig = signer.sign(peer_msg(&id, &peer, &key).as_bytes());
        let _ = (self.doc.get_map(PEERS)).insert(&peer, format!("{key} {}", hex(&sig.to_bytes())));
        self.me = Some(Me { key, signer });
        self.set_my_name(&me.name);
    }

    /// Changes the name others see for this user. Not undoable.
    pub fn set_my_name(&mut self, name: &str) {
        let Some(me) = &self.me else { return };
        let name = clean_name(name);
        let m = member_map(&self.doc, &me.key);
        if get_str(&m, "name").as_deref() != Some(name.as_str()) {
            let _ = m.insert("name", name.as_str());
        }
        self.commit_quietly();
    }

    /// This user's key in this project, once signed in.
    pub fn my_key(&self) -> Option<&str> {
        self.me.as_ref().map(|m| m.key.as_str())
    }

    /// This user's role, if they have one.
    pub fn my_role(&self) -> Option<Role> {
        let key = self.my_key()?;
        self.members().into_iter().find(|m| m.key == key)?.role
    }

    /// Everyone named in the project: the owner first, then members by
    /// name, then people not let in yet. Empty for a project with no
    /// owner.
    pub fn members(&self) -> Vec<Member> {
        let Some(who) = Who::read(&self.doc, None) else {
            return Vec::new();
        };
        let mut out: Vec<Member> = who.members.into_values().collect();
        let rank = |m: &Member| match m.role {
            Some(Role::Owner) => 0,
            Some(_) => 1,
            None => 2,
        };
        out.sort_by(|a, b| {
            (rank(a), a.name.to_lowercase(), &a.key).cmp(&(rank(b), b.name.to_lowercase(), &b.key))
        });
        out
    }

    /// The display name for `key`, if they are named in the project.
    pub fn name_of(&self, key: &str) -> Option<String> {
        let who = Who::read(&self.doc, None)?;
        who.members.get(key).map(Member::shown_name)
    }

    /// Lets `key` in as `role` (editor or commenter), or with `None`
    /// takes their role away. Only the owner can, and the owner's own
    /// role can't be changed. Undoable.
    pub fn set_role(&mut self, key: &str, role: Option<Role>) -> Result<(), String> {
        let me = self.me.as_ref().ok_or("not signed in")?;
        let meta = self.doc.get_map(META);
        let id = get_str(&meta, "id").ok_or("the project has no owner")?;
        if get_str(&meta, "owner").as_deref() != Some(me.key.as_str()) {
            return Err("only the project's owner can change who is in it".into());
        }
        if key == me.key || role == Some(Role::Owner) {
            return Err("the owner stays the owner".into());
        }
        if from_hex(key).is_none_or(|k| k.len() != 32) {
            return Err("not a member's key".into());
        }
        let m = member_map(&self.doc, key);
        match role {
            Some(r) => {
                let sig = me.signer.sign(grant_msg(&id, key, r).as_bytes());
                let _ = m.insert("role", r.as_str());
                let _ = m.insert("grant", hex(&sig.to_bytes()));
            }
            None => {
                let _ = m.delete("role");
                let _ = m.delete("grant");
            }
        }
        self.doc.commit();
        Ok(())
    }

    /// Checks another copy's changes (or a whole copy) against who may
    /// make them, without merging them.
    pub fn check(&self, bytes: &[u8]) -> Result<Check, String> {
        let fork = self.doc.fork();
        crate::doc::import_lenient(&fork, bytes)?;
        Ok(check_new(&self.doc, &fork))
    }

    fn commit_quietly(&mut self) {
        self.doc.set_next_commit_origin(crate::doc::QUIET);
        self.doc.commit();
    }
}

/// Checks the changes `after` has that `before` hasn't.
pub(crate) fn check_new(before: &LoroDoc, after: &LoroDoc) -> Check {
    let mut check = Check::default();
    // The owner as this copy knows them; a copy that has none takes the
    // first owner it hears of.
    let owner = Who::owner_since(before).or_else(|| Who::owner_since(after));
    let Some((id, owner, since)) = owner else {
        return check;
    };
    let before_owner = after.frontiers_to_vv(&since).unwrap_or_default();
    let Some(who) = Who::read(after, Some((&id, &owner))) else {
        return check;
    };
    let base_vv = before.oplog_vv();
    let Ok(json) = after.export_json_updates_without_peer_compression(&base_vv, &after.oplog_vv())
    else {
        check.unknown = true;
        return check;
    };
    let base_comments = comment_authors(before);
    let after_comments = comment_authors(after);
    let mut refused = BTreeSet::new();
    for change in &json.changes {
        if before_owner.includes_id(change.id) {
            continue;
        }
        let Some(key) = who.peers.get(&change.id.peer) else {
            check.unknown = true;
            continue;
        };
        let role = who.members.get(key).and_then(|m| m.role);
        if role == Some(Role::Owner) || refused.contains(key) {
            continue;
        }
        let peer = change.id.peer.to_string();
        for op in &change.ops {
            let Some(path) = op_path(after, &op.container, &op.content) else {
                refused.insert(key.clone());
                break;
            };
            let p: Vec<&str> = path.iter().map(String::as_str).collect();
            let own_comment = |c: &str| match base_comments.get(c) {
                Some(author) => author == key,
                None => after_comments.get(c).is_none_or(|a| a == key),
            };
            let allowed = match p.as_slice() {
                // Anyone may say which run of the app was them, and give
                // their own name.
                [PEERS, p, ..] => *p == peer,
                [MEMBERS, k] => k == key,
                [MEMBERS, k, "name"] => k == key,
                [MEMBERS, ..] | [META, ..] => false,
                [COMMENTS, c, ..] => match role {
                    Some(Role::Editor) => true,
                    Some(Role::Commenter) => own_comment(c),
                    _ => false,
                },
                _ => role == Some(Role::Editor),
            };
            if !allowed {
                refused.insert(key.clone());
                break;
            }
        }
    }
    check.refused = refused
        .into_iter()
        .map(|k| {
            who.members.get(&k).cloned().unwrap_or(Member {
                key: k,
                name: String::new(),
                role: None,
            })
        })
        .collect();
    check
}

/// Who is who, as a document says, with every signature checked.
struct Who {
    members: BTreeMap<String, Member>,
    /// Peer id to the key of the member who signed for it.
    peers: BTreeMap<u64, String>,
}

impl Who {
    /// The project's id and owner, if the owner's signature holds.
    fn owner(doc: &LoroDoc) -> Option<(String, String)> {
        let (id, owner, _) = Who::owner_since(doc)?;
        Some((id, owner))
    }

    /// The project's id and owner, and the history from before it had
    /// one, if the owner's signature holds.
    fn owner_since(doc: &LoroDoc) -> Option<(String, String, Frontiers)> {
        let meta = doc.get_map(META);
        let id = get_str(&meta, "id")?;
        let owner = get_str(&meta, "owner")?;
        let since = get_str(&meta, "since").unwrap_or_default();
        let sig = get_str(&meta, "owner_sig")?;
        let frontiers = Frontiers::decode(&from_hex(&since)?).ok()?;
        verify(&owner, &owner_msg(&id, &owner, &since), &sig).then_some((id, owner, frontiers))
    }

    /// With `owner` given, as that owner sees things; otherwise as
    /// `doc` says.
    fn read(doc: &LoroDoc, owner: Option<(&str, &str)>) -> Option<Who> {
        let (id, owner) = match owner {
            Some((i, o)) => (i.to_string(), o.to_string()),
            None => Who::owner(doc)?,
        };
        let mut members = BTreeMap::new();
        if let LoroValue::Map(ms) = doc.get_map(MEMBERS).get_deep_value() {
            for (key, m) in ms.iter() {
                let LoroValue::Map(m) = m else { continue };
                let s = |k: &str| match m.get(k) {
                    Some(LoroValue::String(v)) => Some(v.to_string()),
                    _ => None,
                };
                let role = s("role").and_then(|r| Role::parse(&r)).filter(|r| {
                    let granted =
                        s("grant").is_some_and(|g| verify(&owner, &grant_msg(&id, key, *r), &g));
                    // The owner's role is the owner's alone.
                    granted && ((*r == Role::Owner) == (key.as_str() == owner))
                });
                members.insert(
                    key.to_string(),
                    Member {
                        key: key.to_string(),
                        name: clean_name(&s("name").unwrap_or_default()),
                        role,
                    },
                );
            }
        }
        let mut peers = BTreeMap::new();
        if let LoroValue::Map(ps) = doc.get_map(PEERS).get_deep_value() {
            for (peer, v) in ps.iter() {
                let (Ok(p), LoroValue::String(v)) = (peer.parse::<u64>(), v) else {
                    continue;
                };
                let Some((key, sig)) = v.split_once(' ') else {
                    continue;
                };
                if verify(key, &peer_msg(&id, peer, key), sig) {
                    peers.insert(p, key.to_string());
                    members.entry(key.to_string()).or_insert_with(|| Member {
                        key: key.to_string(),
                        name: String::new(),
                        role: None,
                    });
                }
            }
        }
        Some(Who { members, peers })
    }
}

/// Comment id to its author's key.
fn comment_authors(doc: &LoroDoc) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let LoroValue::Map(cs) = doc.get_map(COMMENTS).get_deep_value() {
        for (id, c) in cs.iter() {
            if let LoroValue::Map(c) = c
                && let Some(LoroValue::String(a)) = c.get("author")
            {
                out.insert(id.to_string(), a.to_string());
            }
        }
    }
    out
}

/// Where an op writes, as keys from the root: the container's path,
/// then the map key it sets.
fn op_path(doc: &LoroDoc, container: &ContainerID, content: &JsonOpContent) -> Option<Vec<String>> {
    // Mergeable children have root ids, so the path is asked for even
    // for those.
    let mut path: Vec<String> = match doc.get_path_to_container(container) {
        Some(p) if !p.is_empty() => p
            .into_iter()
            .map(|(_, i)| match i {
                Index::Key(k) => k.to_string(),
                Index::Seq(n) => n.to_string(),
                Index::Node(n) => n.to_string(),
            })
            .collect(),
        _ => match container {
            ContainerID::Root { name, .. } => vec![name.to_string()],
            _ => return None,
        },
    };
    if let JsonOpContent::Map(JsonMapOp::Insert { key, .. } | JsonMapOp::Delete { key }) = content {
        path.push(key.clone());
    }
    Some(path)
}

/// A member's map, made if missing. The owner letting someone in while
/// they name themselves get one map, not one each.
fn member_map(doc: &LoroDoc, key: &str) -> LoroMap {
    (doc.get_map(MEMBERS))
        .ensure_mergeable_map(key)
        .unwrap_or_else(|_| LoroMap::new())
}

fn get_str(m: &LoroMap, key: &str) -> Option<String> {
    match m.get(key)?.into_value().ok()? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn owner_msg(id: &str, owner: &str, since: &str) -> String {
    format!("daw owner\n{id}\n{owner}\n{since}")
}

fn grant_msg(id: &str, key: &str, role: Role) -> String {
    format!("daw role\n{id}\n{key}\n{}", role.as_str())
}

fn peer_msg(id: &str, peer: &str, key: &str) -> String {
    format!("daw peer\n{id}\n{peer}\n{key}")
}

fn verify(key: &str, msg: &str, sig: &str) -> bool {
    let (Some(k), Some(s)) = (from_hex(key), from_hex(sig)) else {
        return false;
    };
    let (Ok(k), Ok(s)) = (<[u8; 32]>::try_from(k), <[u8; 64]>::try_from(s)) else {
        return false;
    };
    let Ok(k) = VerifyingKey::from_bytes(&k) else {
        return false;
    };
    k.verify(msg.as_bytes(), &Signature::from_bytes(&s)).is_ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Marker, Project};

    fn person(name: &str) -> Identity {
        Identity::generate(name).unwrap()
    }

    /// `who`'s copy of `doc`, as a new run of the app.
    fn copy(doc: &ProjectDoc, who: &Identity, peer: u64) -> ProjectDoc {
        let mut d = ProjectDoc::from_bytes(&doc.to_bytes(), peer).unwrap();
        d.sign_in(who);
        d
    }

    fn add_marker(doc: &mut ProjectDoc, id: &str) {
        let mut p = doc.get();
        p.markers.push(Marker {
            id: id.into(),
            bar: 2,
            name: id.into(),
        });
        doc.set(&p);
    }

    fn comment(doc: &mut ProjectDoc, id: &str, text: &str) {
        let author = doc.my_key().unwrap().to_string();
        let m = doc
            .doc
            .get_map(COMMENTS)
            .insert_container(id, LoroMap::new())
            .unwrap();
        m.insert("author", author.as_str()).unwrap();
        m.insert("text", text).unwrap();
        doc.doc.commit();
    }

    fn edit_comment(doc: &mut ProjectDoc, id: &str, text: &str) {
        let m = doc.doc.get_map(COMMENTS).get(id).unwrap();
        let m = m.into_container().unwrap().into_map().unwrap();
        m.insert("text", text).unwrap();
        doc.doc.commit();
    }

    /// The owner's project and a teacher's copy of it.
    fn shared() -> (Identity, ProjectDoc, Identity, ProjectDoc) {
        let andy = person("Andy");
        let mut mine = ProjectDoc::new(1);
        mine.sign_in(&andy);
        add_marker(&mut mine, "verse");
        let pat = person("Pat");
        let theirs = copy(&mine, &pat, 2);
        (andy, mine, pat, theirs)
    }

    #[test]
    fn first_to_sign_in_owns_the_project() {
        let (_, mine, _, theirs) = shared();
        assert_eq!(mine.my_role(), Some(Role::Owner));
        assert_eq!(theirs.my_role(), None);
        let names: Vec<_> = theirs
            .members()
            .iter()
            .map(|m| (m.name.clone(), m.role))
            .collect();
        assert_eq!(
            names,
            [
                ("Andy".to_string(), Some(Role::Owner)),
                ("Pat".into(), None)
            ]
        );
    }

    #[test]
    fn strangers_changes_wait_until_let_in() {
        let (_, mut mine, _, mut theirs) = shared();
        add_marker(&mut theirs, "chorus");
        let check = mine.check(&theirs.to_bytes()).unwrap();
        assert_eq!(check.refused.len(), 1);
        assert_eq!(check.refused[0].name, "Pat");
        assert!(mine.merge(&theirs.to_bytes()).unwrap_err().contains("Pat"));
        assert_eq!(mine.get().markers.len(), 1);

        let pat = check.refused[0].key.clone();
        mine.set_role(&pat, Some(Role::Editor)).unwrap();
        mine.merge(&theirs.to_bytes()).unwrap();
        assert_eq!(mine.get().markers.len(), 2);
        assert_eq!(mine.name_of(&pat).as_deref(), Some("Pat"));

        // The teacher takes the owner's copy, with their role in it, and
        // their next changes go straight in.
        theirs.merge(&mine.to_bytes()).unwrap();
        assert_eq!(theirs.my_role(), Some(Role::Editor));
        add_marker(&mut theirs, "solo");
        mine.merge(&theirs.updates_since(&mine.version()).unwrap())
            .unwrap();
        assert_eq!(mine.get().markers.len(), 3);
    }

    #[test]
    fn opening_and_naming_yourself_needs_no_permission() {
        let (_, mut mine, _, theirs) = shared();
        mine.merge(&theirs.to_bytes()).unwrap();
        assert_eq!(mine.members().len(), 2);
    }

    #[test]
    fn editors_cant_change_who_is_in() {
        let (_, mut mine, _, mut theirs) = shared();
        let pat = theirs.my_key().unwrap().to_string();
        mine.set_role(&pat, Some(Role::Editor)).unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        assert!(theirs.set_role("00", Some(Role::Editor)).is_err());

        // Writing the owner's details by hand is refused on merge.
        theirs
            .doc
            .get_map(META)
            .insert("owner", pat.as_str())
            .unwrap();
        theirs.doc.commit();
        assert!(!mine.check(&theirs.to_bytes()).unwrap().ok());

        // So is promoting themselves, even with a role written in.
        let (_, mut mine, _, theirs) = shared();
        member_map(&theirs.doc, &pat_key(&theirs))
            .insert("role", "editor")
            .unwrap();
        theirs.doc.commit();
        assert!(!mine.check(&theirs.to_bytes()).unwrap().ok());
        assert!(mine.merge(&theirs.to_bytes()).is_err());
    }

    fn pat_key(d: &ProjectDoc) -> String {
        d.my_key().unwrap().to_string()
    }

    #[test]
    fn commenters_change_only_their_own_comments() {
        let (_, mut mine, _, mut theirs) = shared();
        mine.set_role(&pat_key(&theirs), Some(Role::Commenter))
            .unwrap();
        comment(&mut mine, "c1", "Watch the timing here");
        theirs.merge(&mine.to_bytes()).unwrap();
        assert_eq!(theirs.my_role(), Some(Role::Commenter));

        comment(&mut theirs, "c2", "Will do");
        edit_comment(&mut theirs, "c2", "Will do!");
        mine.merge(&theirs.to_bytes()).unwrap();

        // Someone else's comment is out of bounds.
        edit_comment(&mut theirs, "c1", "Never mind");
        assert!(!mine.check(&theirs.to_bytes()).unwrap().ok());
    }

    #[test]
    fn commenters_cant_edit_the_song() {
        let (_, mut mine, _, mut theirs) = shared();
        mine.set_role(&pat_key(&theirs), Some(Role::Commenter))
            .unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        let mut p: Project = theirs.get();
        p.tempo_bpm = 90.0;
        theirs.set(&p);
        assert!(mine.merge(&theirs.to_bytes()).is_err());
        assert_eq!(mine.get().tempo_bpm, 120.0);
    }

    #[test]
    fn commenters_cant_add_tab_blocks() {
        let (_, mut mine, _, mut theirs) = shared();
        mine.set_role(&pat_key(&theirs), Some(Role::Commenter))
            .unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        let mut p: Project = theirs.get();
        p.tabs.push(crate::TabBlock {
            id: "t1".into(),
            marker: None,
            title: String::new(),
            text: "e|---|".into(),
            author: pat_key(&theirs),
            created: 0,
        });
        theirs.set(&p);
        assert!(mine.merge(&theirs.to_bytes()).is_err());
        assert!(mine.get().tabs.is_empty());
    }

    #[test]
    fn taking_a_role_away_stops_later_changes() {
        let (_, mut mine, _, mut theirs) = shared();
        let pat = pat_key(&theirs);
        mine.set_role(&pat, Some(Role::Editor)).unwrap();
        theirs.merge(&mine.to_bytes()).unwrap();
        mine.set_role(&pat, None).unwrap();
        add_marker(&mut theirs, "late");
        assert!(mine.merge(&theirs.to_bytes()).is_err());
    }

    #[test]
    fn history_from_before_an_owner_is_kept() {
        // Made before identity: edits by a run that signed nothing.
        let mut old = ProjectDoc::new(7);
        add_marker(&mut old, "intro");
        let andy = person("Andy");
        let mine = copy(&old, &andy, 8);
        assert_eq!(mine.my_role(), Some(Role::Owner));
        // Someone with none of that history takes all of it.
        let mut fresh = ProjectDoc::new(9);
        fresh.merge(&mine.to_bytes()).unwrap();
        assert_eq!(fresh.get().markers.len(), 1);
        // But unsigned changes made since are refused.
        let mut stray = ProjectDoc::from_bytes(&mine.to_bytes(), 10).unwrap();
        add_marker(&mut stray, "stray");
        let check = fresh.check(&stray.to_bytes()).unwrap();
        assert!(check.unknown && !check.ok());
    }

    #[test]
    fn projects_without_an_owner_merge_freely() {
        let mut a = ProjectDoc::new(1);
        let mut b = ProjectDoc::new(2);
        add_marker(&mut b, "x");
        a.merge(&b.to_bytes()).unwrap();
        assert!(a.members().is_empty());
    }

    #[test]
    fn keys_differ_per_project_and_survive_saving() {
        let andy = person(" Andy\n");
        assert_eq!(andy.name, "Andy");
        let again = Identity::from_text(&andy.to_text()).unwrap();
        assert_eq!(again.name, "Andy");
        assert_eq!(again.key_for("p1"), andy.key_for("p1"));
        assert_ne!(andy.key_for("p1"), andy.key_for("p2"));
        assert!(Identity::from_text("name Andy").is_none());
        assert_eq!(clean_name(&"x".repeat(100)).len(), MAX_NAME);
    }

    #[test]
    fn signing_in_again_keeps_the_same_key() {
        let (andy, mine, _, _) = shared();
        let reopened = copy(&mine, &andy, 5);
        assert_eq!(reopened.my_key(), mine.my_key());
        assert_eq!(reopened.my_role(), Some(Role::Owner));
        assert_eq!(reopened.members().len(), 1);
    }
}
