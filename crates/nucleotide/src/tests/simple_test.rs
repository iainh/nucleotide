// ABOUTME: Simple test to verify test infrastructure is working
// ABOUTME: Basic test to validate compilation and test runner setup

#[cfg(test)]
mod tests {

    #[test]
    fn test_infrastructure_works() {
        assert_eq!(2 + 2, 4);
    }

    #[test]
    fn test_std_path_operations() {
        use std::path::{Path, PathBuf};

        let path = PathBuf::from("/home/user/project");
        assert_eq!(path.file_name().unwrap(), "project");

        let parent = path.parent().unwrap();
        assert_eq!(parent, Path::new("/home/user"));
    }
}
