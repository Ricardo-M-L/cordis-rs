//! Group — entry grouping for hierarchical organisation.

/// A Group contains related entries.
#[derive(Debug, Clone)]
pub struct Group {
    name: String,
    entries: Vec<String>,
}

impl Group {
    /// Create a new Group with the given name.
    pub fn new(name: &str) -> Self {
        Group {
            name: name.to_string(),
            entries: Vec::new(),
        }
    }

    /// Add an entry to this group.
    pub fn add_entry(&mut self, entry: &str) {
        self.entries.push(entry.to_string());
    }

    /// Return the group's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return a slice of the group's entries.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the group has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for Group {
    fn default() -> Self {
        Self::new("default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group() {
        let mut group = Group::new("web");
        assert_eq!(group.name(), "web");
        assert!(group.is_empty());

        group.add_entry("app");
        group.add_entry("api");
        assert_eq!(group.len(), 2);
        assert_eq!(group.entries(), vec!["app".to_string(), "api".to_string()]);
    }
}
