//! Scene diff computation for --output-diff

/// Compute a line-by-line diff between two TOML strings
pub fn compute_scene_diff(before: &str, after: &str) -> String {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    let mut output = String::new();
    let mut bi = 0;
    let mut ai = 0;

    while bi < before_lines.len() || ai < after_lines.len() {
        match (before_lines.get(bi), after_lines.get(ai)) {
            (Some(b), Some(a)) if b == a => {
                output.push_str(&format!("  {}\n", b));
                bi += 1;
                ai += 1;
            }
            (Some(b), Some(a)) => {
                // Lines differ — check for context
                // Look ahead for a matching line
                let ahead_match = after_lines[ai..]
                    .iter()
                    .position(|line| before_lines.get(bi) == Some(line));

                if let Some(offset) = ahead_match {
                    if offset <= 3 {
                        // Added lines
                        for line in &after_lines[ai..ai + offset] {
                            output.push_str(&format!("+ {}\n", line));
                        }
                        ai += offset;
                        continue;
                    }
                }

                let ahead_match_b = before_lines[bi..]
                    .iter()
                    .position(|line| after_lines.get(ai) == Some(line));

                if let Some(offset) = ahead_match_b {
                    if offset <= 3 {
                        for line in &before_lines[bi..bi + offset] {
                            output.push_str(&format!("- {}\n", line));
                        }
                        bi += offset;
                        continue;
                    }
                }

                // Simple replacement
                output.push_str(&format!("- {}\n", b));
                output.push_str(&format!("+ {}\n", a));
                bi += 1;
                ai += 1;
            }
            (Some(b), None) => {
                output.push_str(&format!("- {}\n", b));
                bi += 1;
            }
            (None, Some(a)) => {
                output.push_str(&format!("+ {}\n", a));
                ai += 1;
            }
            (None, None) => break,
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_files() {
        let content = "[scene]\nname = \"test\"\n";
        let diff = compute_scene_diff(content, content);
        assert!(!diff.contains('+'));
        assert!(!diff.contains('-'));
    }

    #[test]
    fn test_added_lines() {
        let before = "[scene]\nname = \"test\"\n";
        let after = "[scene]\nname = \"test\"\n\n[entities.new_entity]\narchetype = \"door\"\n";
        let diff = compute_scene_diff(before, after);
        assert!(diff.contains("+ "));
    }

    #[test]
    fn test_removed_lines() {
        let before = "[scene]\nname = \"test\"\n\n[entities.old]\narchetype = \"door\"\n";
        let after = "[scene]\nname = \"test\"\n";
        let diff = compute_scene_diff(before, after);
        assert!(diff.contains("- "));
    }

    #[test]
    fn test_changed_lines() {
        let before = "value = 200.0\n";
        let after = "value = 90.0\n";
        let diff = compute_scene_diff(before, after);
        assert!(diff.contains("- value = 200.0"));
        assert!(diff.contains("+ value = 90.0"));
    }
}
