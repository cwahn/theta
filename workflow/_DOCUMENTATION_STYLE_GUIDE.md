# Theta Documentation Style Guide

This document defines the uniform documentation format for all items in the Theta actor framework codebase. The goal is to provide minimal yet comprehensive documentation that includes essential information and avoids verbosity.

## General Principles

- **Concise and Essential**: Focus on what the item does, not how it's implemented
- **Consistent Structure**: Follow the same pattern for similar item types
- **Executable Examples**: Use working code examples with hidden test setup where possible
- **No Verbose Explanations**: Avoid implementation details and lengthy descriptions
- **Essential Information Only**: Include only what users need to know to use the item effectively

## Item Type Formats

### 1. Crate Root Documentation (`lib.rs`)

**Structure:**
```rust
//! # [Crate Name]: [Brief Description]
//!
//! [2-3 sentence overview of what the crate does]
//!
//! ## Key Features
//!
//! - **[Feature]**: [Brief description]
//! - **[Feature]**: [Brief description]
//! - ...
//!
//! ## Quick Start
//!
//! ```rust
//! [Working example showing basic usage]
//! ```
//!
//! ## Features
//!
//! - **`feature-name`** (default): [Description]
//! - **`feature-name`**: [Description]
//! - ...
//!
//! [Optional: Links to external resources]
```

**Essential Items:**
- Brief crate description (1 line)
- Key features (bullet points)
- Working quick start example
- Feature flags documentation

**Optional Items:**
- Links to external documentation
- Advanced usage patterns

### 2. Module Documentation

**Structure:**
```rust
//! [Brief description of module purpose].
//!
//! [1-2 sentences describing what the module provides]
//!
//! # Core Types
//!
//! - [`Type`] - [Brief description]
//! - [`Type`] - [Brief description]
//! - ...
//!
//! # [Category] (e.g., Communication Patterns, Usage Patterns)
//!
//! ## [Pattern Name]
//! [Brief description]:
//! ```rust
//! [Working example]
//! ```
```

**Essential Items:**
- Brief module purpose
- List of core types with descriptions
- Usage patterns with working examples

**Optional Items:**
- Multiple pattern categories
- Advanced examples

### 3. Struct/Enum Documentation

**Structure:**
```rust
/// [Brief description of what the type represents].
///
/// [1-2 sentences about purpose and key characteristics]
///
/// # [Optional: Usage/Examples]
///
/// ```rust
/// [Simple working example]
/// ```
///
/// # Type Parameters (if generic)
///
/// * `T` - [Description of type parameter]
```

**Essential Items:**
- Brief description (1 line)
- Purpose and characteristics (1-2 sentences)

**Optional Items:**
- Usage examples for complex types
- Type parameter documentation for generics
- Important notes or warnings

### 4. Trait Documentation

**Structure:**
```rust
/// [Brief description of trait purpose].
///
/// [1-2 sentences about what implementing this trait provides]
///
/// # Usage with [Related System/Macro]
///
/// [Brief explanation of how trait is typically used]
///
/// ## [Pattern Name]
///
/// [Description of usage pattern]:
///
/// ```rust
/// [Working example]
/// ```
///
/// # [Category] (e.g., Default Implementations, Key Methods)
///
/// [Brief explanations]
```

**Essential Items:**
- Brief trait purpose
- How trait fits into the system
- Basic usage example

**Optional Items:**
- Usage patterns
- Default implementations
- Integration with other systems

### 5. Method Documentation

**Structure:**
```rust
/// [Brief description of what method does].
///
/// [Optional: 1 sentence about important behavior/notes]
///
/// # Arguments
///
/// * `param` - [Description of parameter purpose/constraints]
/// * `param2` - [Description of parameter purpose/constraints]
///
/// # Returns
///
/// [Description of return value and any important characteristics]
///
/// # Errors (if method can fail)
///
/// [Description of error conditions]
///
/// # Note (if important)
///
/// [Important usage note, warning, or constraint]
```

**Essential Items for PUBLIC methods:**
- Brief description of method purpose (1 line)
- Arguments section (for all parameters, even if obvious from signature)
- Returns section (for all non-unit returns)
- Errors section (if method returns Result)

**Optional Items:**
- Important behavioral notes
- Usage warnings or constraints
- Simple usage examples for complex methods

### 6. Function Documentation

**Structure:**
```rust
/// [Brief description of function purpose].
///
/// [Optional: 1 sentence about key behavior]
///
/// # Arguments
///
/// * `param` - [Description of parameter purpose/constraints]
/// * `param2` - [Description of parameter purpose/constraints]
///
/// # Returns
///
/// [Description of return value and any important characteristics]
///
/// # Errors (if function can fail)
///
/// [Description of error conditions]
///
/// # Examples (for complex functions)
///
/// ```rust
/// [Working example]
/// ```
```

**Essential Items for PUBLIC functions:**
- Brief function purpose
- Arguments section (for all parameters, even if obvious from signature)
- Returns section (for all non-unit returns)
- Errors section (if function returns Result)

**Optional Items:**
- Examples for complex functions
- Error conditions
- Usage notes

### 7. Macro Documentation

**Structure:**
```rust
/// [Brief description of macro purpose].
///
/// [1-2 sentences about what the macro generates/provides]
///
/// # Default Implementations (if applicable)
///
/// The macro automatically provides:
/// - [Item]: [Description]
/// - [Item]: [Description]
///
/// # Usage
///
/// ## [Pattern Name]
/// ```rust
/// [Working example]
/// ```
///
/// ## [Pattern Name]
/// ```rust
/// [Working example]
/// ```
///
/// # Arguments
///
/// [Description of required arguments and optional parameters]
```

**Essential Items:**
- Brief macro purpose
- What the macro generates/provides
- Usage examples for different patterns
- Required arguments

**Optional Items:**
- Default implementations
- Multiple usage patterns
- Optional parameters

### 8. Type Alias Documentation

**Structure:**
```rust
/// [Brief description of what the alias represents].
///
/// [Optional: 1 sentence about when to use it]
```

**Essential Items:**
- Brief description of alias purpose

**Optional Items:**
- When to use the alias vs. the underlying type

### 9. Constant Documentation

**Structure:**
```rust
/// [Brief description of constant purpose/value].
```

**Essential Items:**
- Brief description of what the constant represents

## Code Example Guidelines

### Hidden Test Setup

Use `# ` comments to hide boilerplate while showing clean examples:

```rust
/// ```rust
/// # use theta::prelude::*;
/// # #[derive(Debug, Clone, ActorArgs)]
/// # struct MyActor;
/// # #[actor("uuid")]
/// # impl Actor for MyActor {}
/// let actor = ctx.spawn(MyActor);
/// ```
```

### Working Examples

- All examples should compile and demonstrate real usage
- Use `ignore` only when examples require complex runtime setup
- Prefer compile-only examples over complex async examples
- Show the most common usage pattern first

### Example Complexity

- **Simple**: Method calls, basic construction
- **Medium**: Complete actor definitions, message handling
- **Complex**: Full applications with multiple actors (use sparingly)

## Documentation Anti-Patterns

### Avoid These:

- **Verbose explanations**: Implementation details and lengthy descriptions
- **Redundant sections**: Repeating information already in type signatures
- **Complex examples**: Examples requiring extensive setup
- **Implementation notes**: How something works internally
- **Excessive use cases**: Long lists of when to use something

### Instead:

- **Essential information**: What users need to know to use it
- **Clear examples**: Working code demonstrating usage
- **Concise descriptions**: One-line summaries of purpose
- **Important notes**: Only critical warnings or constraints

## Review Checklist

Before committing documentation:

- [ ] Brief, one-line description of purpose
- [ ] Essential information only (no implementation details)
- [ ] Working code examples (when applicable)
- [ ] Consistent format for item type
- [ ] No verbose explanations or redundant sections
- [ ] Important warnings/constraints included
- [ ] Examples compile and demonstrate real usage
- [ ] Hidden test setup used for clean examples
