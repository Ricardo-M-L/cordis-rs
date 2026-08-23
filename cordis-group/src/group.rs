//! Group — simple entry grouping.

/// A Group organises entries under a common name.
#[derive(Debug, Clone)]
pub struct Group {
    name: String,
    entries: Vec<String>,
}

impl Group {
    pub fn new(name: &str) -> Self {
        Group {
            name: name.to_string(),
            entries: Vec::new(),
        }
    }

    pub fn add_entry(&mut self, entry: &str) {
        self.entries.push(entry.to_string());
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group() {
        let mut group = Group::new("web");
        group.add_entry("app");
        group.add_entry("api");
        assert_eq!(group.name(), "web");
        assert_eq!(group.len(), 2);
    }
}
