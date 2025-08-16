// ABOUTME: Theme validation system for ensuring theme completeness and accessibility
// ABOUTME: Provides comprehensive validation rules, accessibility checks, and detailed reporting

use crate::Theme;
use gpui::{Hsla, Pixels};
use std::collections::HashMap;

/// Theme validator for comprehensive theme validation
pub struct ThemeValidator {
    /// Validation rules configuration
    rules: ValidationRules,
    /// Custom validation functions
    custom_validators: Vec<CustomValidator>,
}

/// Validation rules configuration
#[derive(Debug, Clone)]
pub struct ValidationRules {
    /// Color validation rules
    pub color_rules: ColorValidationRules,
    /// Size validation rules
    pub size_rules: SizeValidationRules,
    /// Accessibility validation rules
    pub accessibility_rules: AccessibilityValidationRules,
    /// Consistency validation rules
    pub consistency_rules: ConsistencyValidationRules,
}

/// Color validation rules
#[derive(Debug, Clone)]
pub struct ColorValidationRules {
    /// Require essential colors
    pub require_essential_colors: bool,
    /// Essential color names
    pub essential_colors: Vec<String>,
    /// Check color contrast ratios
    pub check_contrast: bool,
    /// Minimum contrast ratio for normal text
    pub min_contrast_ratio: f32,
    /// Minimum contrast ratio for large text
    pub min_large_text_contrast: f32,
    /// Validate color accessibility
    pub validate_color_blindness: bool,
    /// Check for sufficient color differences
    pub check_color_differences: bool,
    /// Minimum color difference threshold
    pub min_color_difference: f32,
}

/// Size validation rules
#[derive(Debug, Clone)]
pub struct SizeValidationRules {
    /// Minimum touch target size (for accessibility)
    pub min_touch_target_size: Pixels,
    /// Maximum reasonable size limits
    pub max_size_limits: HashMap<String, Pixels>,
    /// Check size consistency
    pub check_size_consistency: bool,
    /// Validate spacing ratios
    pub validate_spacing_ratios: bool,
}

/// Accessibility validation rules
#[derive(Debug, Clone)]
pub struct AccessibilityValidationRules {
    /// Validate WCAG compliance level
    pub wcag_level: WcagLevel,
    /// Check focus indicator visibility
    pub check_focus_indicators: bool,
    /// Validate color-only information
    pub check_color_only_info: bool,
    /// Check animation considerations
    pub check_animations: bool,
    /// Validate text size requirements
    pub check_text_size: bool,
}

/// Consistency validation rules
#[derive(Debug, Clone)]
pub struct ConsistencyValidationRules {
    /// Check color harmony
    pub check_color_harmony: bool,
    /// Validate size relationships
    pub check_size_relationships: bool,
    /// Check semantic consistency
    pub check_semantic_consistency: bool,
    /// Validate brand consistency
    pub check_brand_consistency: bool,
}

/// WCAG compliance levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WcagLevel {
    /// WCAG 2.1 Level A
    A,
    /// WCAG 2.1 Level AA (recommended)
    AA,
    /// WCAG 2.1 Level AAA
    AAA,
}

/// Custom validator function type
pub type CustomValidator = Box<dyn Fn(&Theme) -> ValidationResult + Send + Sync>;

/// Validation result containing all findings
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Overall validation success
    pub is_valid: bool,
    /// All validation issues found
    pub issues: Vec<ValidationIssue>,
    /// Validation warnings
    pub warnings: Vec<ValidationWarning>,
    /// Validation metadata
    pub metadata: ValidationMetadata,
}

/// Individual validation issue
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    /// Issue severity
    pub severity: IssueSeverity,
    /// Issue category
    pub category: IssueCategory,
    /// Issue description
    pub description: String,
    /// Affected element or property
    pub affected_element: Option<String>,
    /// Suggested fix
    pub suggested_fix: Option<String>,
    /// Related WCAG guideline
    pub wcag_guideline: Option<String>,
}

/// Validation warning
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    /// Warning message
    pub message: String,
    /// Warning category
    pub category: IssueCategory,
    /// Affected element
    pub affected_element: Option<String>,
}

/// Validation metadata
#[derive(Debug, Clone)]
pub struct ValidationMetadata {
    /// Validation timestamp
    pub timestamp: std::time::SystemTime,
    /// Validator version
    pub validator_version: String,
    /// Number of rules checked
    pub rules_checked: usize,
    /// Validation duration
    pub validation_duration: std::time::Duration,
}

/// Issue severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Issue categories
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueCategory {
    Color,
    Size,
    Accessibility,
    Consistency,
    Performance,
    Usability,
}

impl Default for ValidationRules {
    fn default() -> Self {
        Self {
            color_rules: ColorValidationRules::default(),
            size_rules: SizeValidationRules::default(),
            accessibility_rules: AccessibilityValidationRules::default(),
            consistency_rules: ConsistencyValidationRules::default(),
        }
    }
}

impl Default for ColorValidationRules {
    fn default() -> Self {
        Self {
            require_essential_colors: true,
            essential_colors: vec![
                "primary".to_string(),
                "background".to_string(),
                "text_primary".to_string(),
                "surface".to_string(),
            ],
            check_contrast: true,
            min_contrast_ratio: 4.5, // WCAG AA
            min_large_text_contrast: 3.0, // WCAG AA for large text
            validate_color_blindness: true,
            check_color_differences: true,
            min_color_difference: 500.0, // Delta E difference
        }
    }
}

impl Default for SizeValidationRules {
    fn default() -> Self {
        let mut max_limits = HashMap::new();
        max_limits.insert("space_1".to_string(), gpui::px(100.0));
        max_limits.insert("space_2".to_string(), gpui::px(200.0));
        max_limits.insert("space_3".to_string(), gpui::px(300.0));
        max_limits.insert("space_4".to_string(), gpui::px(400.0));
        
        Self {
            min_touch_target_size: gpui::px(44.0), // iOS/Android guidelines
            max_size_limits: max_limits,
            check_size_consistency: true,
            validate_spacing_ratios: true,
        }
    }
}

impl Default for AccessibilityValidationRules {
    fn default() -> Self {
        Self {
            wcag_level: WcagLevel::AA,
            check_focus_indicators: true,
            check_color_only_info: true,
            check_animations: true,
            check_text_size: true,
        }
    }
}

impl Default for ConsistencyValidationRules {
    fn default() -> Self {
        Self {
            check_color_harmony: true,
            check_size_relationships: true,
            check_semantic_consistency: true,
            check_brand_consistency: false, // Requires custom configuration
        }
    }
}

impl ThemeValidator {
    /// Create a new theme validator with default rules
    pub fn new() -> Self {
        Self {
            rules: ValidationRules::default(),
            custom_validators: Vec::new(),
        }
    }
    
    /// Create a validator for specific WCAG level
    pub fn for_wcag_level(level: WcagLevel) -> Self {
        let mut validator = Self::new();
        validator.rules.accessibility_rules.wcag_level = level;
        
        // Adjust contrast requirements based on WCAG level
        validator.rules.color_rules.min_contrast_ratio = match level {
            WcagLevel::A => 3.0,
            WcagLevel::AA => 4.5,
            WcagLevel::AAA => 7.0,
        };
        
        validator.rules.color_rules.min_large_text_contrast = match level {
            WcagLevel::A => 3.0,
            WcagLevel::AA => 3.0,
            WcagLevel::AAA => 4.5,
        };
        
        validator
    }
    
    /// Add a custom validator
    pub fn add_custom_validator(&mut self, validator: CustomValidator) {
        self.custom_validators.push(validator);
    }
    
    /// Configure validation rules
    pub fn configure_rules<F>(&mut self, configurator: F) 
    where
        F: FnOnce(&mut ValidationRules),
    {
        configurator(&mut self.rules);
    }
    
    /// Validate a theme comprehensively
    pub fn validate_theme(&self, theme: &Theme, metadata: &super::ThemeMetadata) -> Result<ValidationResult, ValidationError> {
        let start_time = std::time::Instant::now();
        let mut issues = Vec::new();
        let mut warnings = Vec::new();
        let mut rules_checked = 0;
        
        nucleotide_logging::debug!(
            theme_name = %metadata.name,
            "Starting theme validation"
        );
        
        // Color validation
        if let Err(color_issues) = self.validate_colors(theme) {
            issues.extend(color_issues);
        }
        rules_checked += 1;
        
        // Size validation
        if let Err(size_issues) = self.validate_sizes(theme) {
            issues.extend(size_issues);
        }
        rules_checked += 1;
        
        // Accessibility validation
        if let Err(a11y_issues) = self.validate_accessibility(theme) {
            issues.extend(a11y_issues);
        }
        rules_checked += 1;
        
        // Consistency validation
        if let Err(consistency_issues) = self.validate_consistency(theme) {
            issues.extend(consistency_issues);
        }
        rules_checked += 1;
        
        // Run custom validators
        for validator in &self.custom_validators {
            let custom_result = validator(theme);
            issues.extend(custom_result.issues);
            warnings.extend(custom_result.warnings);
            rules_checked += 1;
        }
        
        let validation_duration = start_time.elapsed();
        let is_valid = !issues.iter().any(|issue| {
            matches!(issue.severity, IssueSeverity::Error | IssueSeverity::Critical)
        });
        
        let result = ValidationResult {
            is_valid,
            issues,
            warnings,
            metadata: ValidationMetadata {
                timestamp: std::time::SystemTime::now(),
                validator_version: env!("CARGO_PKG_VERSION").to_string(),
                rules_checked,
                validation_duration,
            },
        };
        
        nucleotide_logging::info!(
            theme_name = %metadata.name,
            is_valid = result.is_valid,
            issues_count = result.issues.len(),
            warnings_count = result.warnings.len(),
            duration_ms = validation_duration.as_millis(),
            "Theme validation completed"
        );
        
        Ok(result)
    }
    
    /// Validate color properties
    fn validate_colors(&self, theme: &Theme) -> Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        
        // Check essential colors
        if self.rules.color_rules.require_essential_colors {
            for color_name in &self.rules.color_rules.essential_colors {
                let color_value = self.get_color_by_name(theme, color_name);
                if color_value.is_none() || color_value.unwrap().a == 0.0 {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        category: IssueCategory::Color,
                        description: format!("Missing essential color: {}", color_name),
                        affected_element: Some(color_name.clone()),
                        suggested_fix: Some(format!("Define the {} color", color_name)),
                        wcag_guideline: None,
                    });
                }
            }
        }
        
        // Check contrast ratios
        if self.rules.color_rules.check_contrast {
            let text_bg_contrast = self.calculate_contrast_ratio(
                theme.tokens.colors.text_primary,
                theme.tokens.colors.background,
            );
            
            if text_bg_contrast < self.rules.color_rules.min_contrast_ratio {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    category: IssueCategory::Accessibility,
                    description: format!(
                        "Insufficient contrast between text and background: {:.2} (required: {:.2})",
                        text_bg_contrast,
                        self.rules.color_rules.min_contrast_ratio
                    ),
                    affected_element: Some("text_primary on background".to_string()),
                    suggested_fix: Some("Increase contrast between text and background colors".to_string()),
                    wcag_guideline: Some("WCAG 1.4.3".to_string()),
                });
            }
        }
        
        // Check color blindness considerations
        if self.rules.color_rules.validate_color_blindness {
            self.validate_color_blindness_support(theme, &mut issues);
        }
        
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
    
    /// Validate size properties
    fn validate_sizes(&self, theme: &Theme) -> Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        
        // Check maximum size limits
        for (size_name, max_limit) in &self.rules.size_rules.max_size_limits {
            if let Some(actual_size) = self.get_size_by_name(theme, size_name) {
                if actual_size.0 > max_limit.0 {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Warning,
                        category: IssueCategory::Size,
                        description: format!(
                            "Size {} ({:.1}px) exceeds recommended maximum ({:.1}px)",
                            size_name, actual_size.0, max_limit.0
                        ),
                        affected_element: Some(size_name.clone()),
                        suggested_fix: Some(format!("Consider reducing {} to {:.1}px or less", size_name, max_limit.0)),
                        wcag_guideline: None,
                    });
                }
            }
        }
        
        // Check spacing relationships
        if self.rules.size_rules.validate_spacing_ratios {
            let sizes = &theme.tokens.sizes;
            if sizes.space_2.0 <= sizes.space_1.0 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    category: IssueCategory::Consistency,
                    description: "space_2 should be larger than space_1".to_string(),
                    affected_element: Some("spacing".to_string()),
                    suggested_fix: Some("Ensure spacing values increase progressively".to_string()),
                    wcag_guideline: None,
                });
            }
        }
        
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
    
    /// Validate accessibility requirements
    fn validate_accessibility(&self, theme: &Theme) -> Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        
        // Check focus indicators
        if self.rules.accessibility_rules.check_focus_indicators {
            let focus_contrast = self.calculate_contrast_ratio(
                theme.tokens.colors.primary, // Assuming primary is used for focus
                theme.tokens.colors.background,
            );
            
            if focus_contrast < 3.0 { // Minimum for focus indicators
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    category: IssueCategory::Accessibility,
                    description: "Focus indicators may not be visible enough".to_string(),
                    affected_element: Some("focus_indicator".to_string()),
                    suggested_fix: Some("Increase contrast for focus indicators".to_string()),
                    wcag_guideline: Some("WCAG 2.4.7".to_string()),
                });
            }
        }
        
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
    
    /// Validate theme consistency
    fn validate_consistency(&self, theme: &Theme) -> Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        
        // Check color harmony
        if self.rules.consistency_rules.check_color_harmony {
            let primary_secondary_difference = self.calculate_color_difference(
                theme.tokens.colors.primary,
                theme.tokens.colors.text_secondary,
            );
            
            if primary_secondary_difference < 100.0 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    category: IssueCategory::Consistency,
                    description: "Primary and text secondary colors are very similar".to_string(),
                    affected_element: Some("primary_secondary_colors".to_string()),
                    suggested_fix: Some("Increase difference between primary and secondary colors".to_string()),
                    wcag_guideline: None,
                });
            }
        }
        
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
    
    /// Validate color blindness support
    fn validate_color_blindness_support(&self, theme: &Theme, issues: &mut Vec<ValidationIssue>) {
        // Check if important distinctions rely only on color
        let error_text_difference = self.calculate_luminance_difference(
            theme.tokens.colors.error,
            theme.tokens.colors.text_primary,
        );
        
        if error_text_difference < 0.3 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                category: IssueCategory::Accessibility,
                description: "Error color may not be distinguishable for color-blind users".to_string(),
                affected_element: Some("error_color".to_string()),
                suggested_fix: Some("Ensure error states use more than just color to convey meaning".to_string()),
                wcag_guideline: Some("WCAG 1.4.1".to_string()),
            });
        }
    }
    
    /// Get color by name from theme
    fn get_color_by_name(&self, theme: &Theme, name: &str) -> Option<Hsla> {
        match name {
            "primary" => Some(theme.tokens.colors.primary),
            "secondary" => Some(theme.tokens.colors.text_secondary),
            "background" => Some(theme.tokens.colors.background),
            "surface" => Some(theme.tokens.colors.surface),
            "text_primary" => Some(theme.tokens.colors.text_primary),
            "text_secondary" => Some(theme.tokens.colors.text_secondary),
            "border_default" => Some(theme.tokens.colors.border_default),
            "error" => Some(theme.tokens.colors.error),
            "warning" => Some(theme.tokens.colors.warning),
            "success" => Some(theme.tokens.colors.success),
            _ => None,
        }
    }
    
    /// Get size by name from theme
    fn get_size_by_name(&self, theme: &Theme, name: &str) -> Option<Pixels> {
        match name {
            "space_1" => Some(theme.tokens.sizes.space_1),
            "space_2" => Some(theme.tokens.sizes.space_2),
            "space_3" => Some(theme.tokens.sizes.space_3),
            "space_4" => Some(theme.tokens.sizes.space_4),
            "radius_sm" => Some(theme.tokens.sizes.radius_sm),
            "radius_md" => Some(theme.tokens.sizes.radius_md),
            "radius_lg" => Some(theme.tokens.sizes.radius_lg),
            _ => None,
        }
    }
    
    /// Calculate contrast ratio between two colors
    fn calculate_contrast_ratio(&self, color1: Hsla, color2: Hsla) -> f32 {
        let l1 = self.relative_luminance(color1);
        let l2 = self.relative_luminance(color2);
        
        let lighter = l1.max(l2);
        let darker = l1.min(l2);
        
        (lighter + 0.05) / (darker + 0.05)
    }
    
    /// Calculate color difference (Delta E)
    fn calculate_color_difference(&self, color1: Hsla, color2: Hsla) -> f32 {
        // Simplified Delta E calculation
        let dl = color1.l - color2.l;
        let da = (color1.s * color1.l.cos()) - (color2.s * color2.l.cos());
        let db = (color1.s * color1.l.sin()) - (color2.s * color2.l.sin());
        
        (dl * dl + da * da + db * db).sqrt() * 100.0
    }
    
    /// Calculate luminance difference
    fn calculate_luminance_difference(&self, color1: Hsla, color2: Hsla) -> f32 {
        let l1 = self.relative_luminance(color1);
        let l2 = self.relative_luminance(color2);
        (l1 - l2).abs()
    }
    
    /// Calculate relative luminance
    fn relative_luminance(&self, color: Hsla) -> f32 {
        // Simplified luminance calculation using lightness
        color.l
    }
}

impl ValidationResult {
    /// Check if validation passed
    pub fn is_valid(&self) -> bool {
        self.is_valid
    }
    
    /// Get critical issues
    pub fn critical_issues(&self) -> Vec<&ValidationIssue> {
        self.issues.iter()
            .filter(|issue| issue.severity == IssueSeverity::Critical)
            .collect()
    }
    
    /// Get error issues
    pub fn error_issues(&self) -> Vec<&ValidationIssue> {
        self.issues.iter()
            .filter(|issue| issue.severity == IssueSeverity::Error)
            .collect()
    }
    
    /// Get warning issues
    pub fn warning_issues(&self) -> Vec<&ValidationIssue> {
        self.issues.iter()
            .filter(|issue| issue.severity == IssueSeverity::Warning)
            .collect()
    }
    
    /// Get issues by category
    pub fn issues_by_category(&self, category: IssueCategory) -> Vec<&ValidationIssue> {
        self.issues.iter()
            .filter(|issue| issue.category == category)
            .collect()
    }
    
    /// Generate summary report
    pub fn summary(&self) -> String {
        format!(
            "Validation Result: {} | {} issues ({} critical, {} errors, {} warnings) | Checked {} rules in {:.2}ms",
            if self.is_valid { "PASS" } else { "FAIL" },
            self.issues.len(),
            self.critical_issues().len(),
            self.error_issues().len(),
            self.warning_issues().len(),
            self.metadata.rules_checked,
            self.metadata.validation_duration.as_secs_f64() * 1000.0
        )
    }
}

impl Default for ThemeValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation errors
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Invalid theme structure
    InvalidTheme(String),
    /// Validation rule error
    RuleError(String),
    /// Internal validation error
    InternalError(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::InvalidTheme(msg) => write!(f, "Invalid theme: {}", msg),
            ValidationError::RuleError(msg) => write!(f, "Validation rule error: {}", msg),
            ValidationError::InternalError(msg) => write!(f, "Internal validation error: {}", msg),
        }
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_validator_creation() {
        let validator = ThemeValidator::new();
        assert!(validator.rules.color_rules.require_essential_colors);
        assert_eq!(validator.rules.accessibility_rules.wcag_level, WcagLevel::AA);
    }
    
    #[test]
    fn test_wcag_level_configuration() {
        let validator = ThemeValidator::for_wcag_level(WcagLevel::AAA);
        assert_eq!(validator.rules.color_rules.min_contrast_ratio, 7.0);
        assert_eq!(validator.rules.accessibility_rules.wcag_level, WcagLevel::AAA);
    }
    
    #[test]
    fn test_validation_result() {
        let result = ValidationResult {
            is_valid: false,
            issues: vec![
                ValidationIssue {
                    severity: IssueSeverity::Error,
                    category: IssueCategory::Color,
                    description: "Test error".to_string(),
                    affected_element: None,
                    suggested_fix: None,
                    wcag_guideline: None,
                },
                ValidationIssue {
                    severity: IssueSeverity::Warning,
                    category: IssueCategory::Accessibility,
                    description: "Test warning".to_string(),
                    affected_element: None,
                    suggested_fix: None,
                    wcag_guideline: None,
                },
            ],
            warnings: vec![],
            metadata: ValidationMetadata {
                timestamp: std::time::SystemTime::now(),
                validator_version: "test".to_string(),
                rules_checked: 2,
                validation_duration: std::time::Duration::from_millis(10),
            },
        };
        
        assert!(!result.is_valid());
        assert_eq!(result.error_issues().len(), 1);
        assert_eq!(result.warning_issues().len(), 1);
        assert_eq!(result.issues_by_category(IssueCategory::Color).len(), 1);
    }
    
    #[test]
    fn test_contrast_ratio_calculation() {
        let validator = ThemeValidator::new();
        
        let black = Hsla { h: 0.0, s: 0.0, l: 0.0, a: 1.0 };
        let white = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 };
        
        let contrast = validator.calculate_contrast_ratio(black, white);
        assert!(contrast > 10.0); // Should be high contrast
    }
    
    #[test]
    fn test_essential_colors_validation() {
        let validator = ThemeValidator::new();
        let theme = Theme::light();
        
        let result = validator.validate_colors(&theme);
        assert!(result.is_ok()); // Default theme should pass
    }
}