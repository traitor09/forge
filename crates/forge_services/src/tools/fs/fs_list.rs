use std::path::Path;

use anyhow::Context;
use forge_domain::{
    ExecutableTool, FSListInput, NamedTool, ToolCallContext, ToolDescription, ToolName, ToolOutput,
};
use forge_tool_macros::ToolDescription;
use forge_walker::Walker;

use crate::utils::assert_absolute_path;

/// Request to list files and directories within the specified directory. If
/// recursive is true, it will list all files and directories recursively. If
/// recursive is false or not provided, it will only list the top-level
/// contents. The path must be absolute. Do not use this tool to confirm the
/// existence of files you may have created, as the user will let you know if
/// the files were created successfully or not.
#[derive(Default, ToolDescription)]
pub struct FSList {
    sorted: bool,
}

impl NamedTool for FSList {
    fn tool_name() -> ToolName {
        ToolName::new("forge_tool_fs_list")
    }
}

#[async_trait::async_trait]
impl ExecutableTool for FSList {
    type Input = FSListInput;

    async fn call(
        &self,
        _context: &mut ToolCallContext,
        input: Self::Input,
    ) -> anyhow::Result<ToolOutput> {
        let dir = Path::new(&input.path);
        assert_absolute_path(dir)?;

        if !dir.exists() {
            return Err(anyhow::anyhow!("Directory '{}' does not exist", input.path));
        }

        let mut paths = Vec::new();
        let recursive = input.recursive.unwrap_or(false);
        let max_depth = if recursive { usize::MAX } else { 1 };

        let walker = Walker::max_all()
            .cwd(dir.to_path_buf())
            .max_depth(max_depth);

        let mut files = walker
            .get()
            .await
            .with_context(|| format!("Failed to read directory contents from '{}'", input.path))?;

        // Sort the files for consistent snapshots
        if self.sorted {
            files.sort_by(|a, b| a.path.cmp(&b.path));
        }

        for entry in files {
            // Skip the root directory itself
            if entry.path == dir.to_string_lossy() {
                continue;
            }

            if !entry.path.is_empty() {
                if entry.is_dir() {
                    paths.push(format!("<dir path=\"{}\">", entry.path));
                } else {
                    paths.push(format!("<file path=\"{}\">", entry.path));
                };
            }
        }

        Ok(ToolOutput::text(format!(
            "<file_list path=\"{}\">
{}
</file_list>",
            input.path,
            paths.join("\n")
        )))
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;
    use tokio::fs;

    use super::*;
    use crate::utils::{TempDir, ToolContentExtension};

    impl FSList {
        fn new(sorted: bool) -> Self {
            Self { sorted }
        }
    }

    #[tokio::test]
    async fn test_fs_list_empty_directory() {
        let temp_dir = TempDir::new().unwrap();

        let fs_list = FSList::new(true);
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    explanation: None,
                    path: temp_dir.path().to_string_lossy().to_string(),
                    recursive: None,
                },
            )
            .await
            .unwrap()
            .into_string();

        assert_snapshot!(TempDir::normalize(result.as_str()));
    }

    #[tokio::test]
    async fn test_fs_list_with_files_and_dirs() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(temp_dir.path().join("file1.txt"), "content1")
            .await
            .unwrap();
        fs::write(temp_dir.path().join("file2.txt"), "content2")
            .await
            .unwrap();
        fs::create_dir(temp_dir.path().join("dir1")).await.unwrap();
        fs::create_dir(temp_dir.path().join("dir2")).await.unwrap();

        let fs_list = FSList::new(true);
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    explanation: None,
                    path: temp_dir.path().to_string_lossy().to_string(),
                    recursive: None,
                },
            )
            .await
            .unwrap()
            .into_string();

        assert_snapshot!(TempDir::normalize(result.as_str()));
    }

    #[tokio::test]
    async fn test_fs_list_nonexistent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let nonexistent_dir = temp_dir.path().join("nonexistent");

        let fs_list = FSList::new(true);
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    explanation: None,
                    path: nonexistent_dir.to_string_lossy().to_string(),
                    recursive: None,
                },
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fs_list_with_hidden_files() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(temp_dir.path().join("regular.txt"), "content")
            .await
            .unwrap();
        fs::write(temp_dir.path().join(".hidden"), "content")
            .await
            .unwrap();
        fs::create_dir(temp_dir.path().join(".hidden_dir"))
            .await
            .unwrap();

        let fs_list = FSList::new(true);
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    explanation: None,
                    path: temp_dir.path().to_string_lossy().to_string(),
                    recursive: None,
                },
            )
            .await
            .unwrap()
            .into_string();

        assert!(result.contains("regular.txt"));
        assert!(!result.contains(".hidden"));
        assert!(!result.contains(".hidden_dir"));
    }

    #[tokio::test]
    async fn test_fs_list_recursive() {
        let temp_dir = TempDir::new().unwrap();

        // Create nested directory structure
        fs::create_dir(temp_dir.path().join("dir1")).await.unwrap();
        fs::write(temp_dir.path().join("dir1/file1.txt"), "content1")
            .await
            .unwrap();
        fs::create_dir(temp_dir.path().join("dir1/subdir"))
            .await
            .unwrap();
        fs::write(temp_dir.path().join("dir1/subdir/file2.txt"), "content2")
            .await
            .unwrap();
        fs::write(temp_dir.path().join("root.txt"), "content3")
            .await
            .unwrap();

        let fs_list = FSList::new(true);

        // Test recursive listing
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    explanation: None,
                    path: temp_dir.path().to_string_lossy().to_string(),
                    recursive: Some(true),
                },
            )
            .await
            .unwrap()
            .into_string();

        assert_snapshot!(TempDir::normalize(result.as_str()));
    }

    #[tokio::test]
    async fn test_fs_list_relative_path() {
        let fs_list = FSList::new(true);
        let result = fs_list
            .call(
                &mut ToolCallContext::default(),
                FSListInput {
                    path: "relative/path".to_string(),
                    recursive: None,
                    explanation: None,
                },
            )
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Path must be absolute"));
    }
}
