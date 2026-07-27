use super::oid::Oid;

/// A point in time as Git records it: seconds since the Unix epoch plus the
/// author's UTC offset. The domain deliberately avoids a date-time crate —
/// formatting is a presentation concern and belongs in the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub seconds: i64,
    pub offset_minutes: i32,
}

impl Timestamp {
    pub const fn new(seconds: i64, offset_minutes: i32) -> Self {
        Self {
            seconds,
            offset_minutes,
        }
    }

    pub const fn from_utc(seconds: i64) -> Self {
        Self::new(seconds, 0)
    }

    /// Seconds shifted into the author's local wall clock. Only useful for
    /// display; never compare two commits with this.
    pub const fn local_seconds(&self) -> i64 {
        self.seconds + (self.offset_minutes as i64) * 60
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub at: Timestamp,
}

impl Signature {
    pub fn new(name: impl Into<String>, email: impl Into<String>, at: Timestamp) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            at,
        }
    }

    /// Initials for the avatar chip in the commit list.
    pub fn initials(&self) -> String {
        self.name
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .take(2)
            .flat_map(|c| c.to_uppercase())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub summary: String,
    pub body: String,
    pub author: Signature,
    pub committer: Signature,
}

impl Commit {
    pub fn is_root(&self) -> bool {
        self.parents.is_empty()
    }

    /// A merge is any commit with more than one parent. The graph renderer
    /// needs this to decide where lanes converge.
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// The parent a commit's own lane continues from. Git convention: the
    /// first parent is the branch that was merged *into*.
    pub fn first_parent(&self) -> Option<Oid> {
        self.parents.first().copied()
    }

    /// Ordering key for the graph. Commit time, not author time — author time
    /// can move backwards under rebase and would scramble the layout.
    pub fn sort_time(&self) -> i64 {
        self.committer.at.seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    fn signature() -> Signature {
        Signature::new("Ada Lovelace", "ada@example.com", Timestamp::from_utc(1_000))
    }

    fn commit_with_parents(parents: Vec<Oid>) -> Commit {
        Commit {
            id: oid(0xff),
            parents,
            summary: "work".into(),
            body: String::new(),
            author: signature(),
            committer: signature(),
        }
    }

    #[test]
    fn a_commit_without_parents_is_a_root() {
        let commit = commit_with_parents(vec![]);
        assert!(commit.is_root());
        assert!(!commit.is_merge());
        assert_eq!(commit.first_parent(), None);
    }

    #[test]
    fn a_commit_with_one_parent_is_neither_root_nor_merge() {
        let commit = commit_with_parents(vec![oid(1)]);
        assert!(!commit.is_root());
        assert!(!commit.is_merge());
        assert_eq!(commit.first_parent(), Some(oid(1)));
    }

    #[test]
    fn a_commit_with_two_or_more_parents_is_a_merge() {
        let commit = commit_with_parents(vec![oid(1), oid(2)]);
        assert!(commit.is_merge());
        assert_eq!(commit.first_parent(), Some(oid(1)));
    }

    #[test]
    fn sorting_uses_committer_time_not_author_time() {
        let mut commit = commit_with_parents(vec![]);
        commit.author.at = Timestamp::from_utc(50);
        commit.committer.at = Timestamp::from_utc(900);
        assert_eq!(commit.sort_time(), 900);
    }

    #[test]
    fn local_seconds_applies_the_authors_offset() {
        // UTC-03:00, the offset Git records for Buenos Aires.
        let stamp = Timestamp::new(1_000, -180);
        assert_eq!(stamp.local_seconds(), 1_000 - 10_800);
    }

    #[test]
    fn initials_take_the_first_two_words() {
        assert_eq!(signature().initials(), "AL");
    }

    #[test]
    fn initials_handle_a_single_word_name() {
        let sig = Signature::new("torvalds", "t@example.com", Timestamp::from_utc(0));
        assert_eq!(sig.initials(), "T");
    }

    #[test]
    fn initials_of_an_empty_name_are_empty() {
        let sig = Signature::new("", "t@example.com", Timestamp::from_utc(0));
        assert_eq!(sig.initials(), "");
    }
}
