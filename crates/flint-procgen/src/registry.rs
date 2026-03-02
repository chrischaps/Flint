//! Generator registry — maps type names to generator implementations.
//!
//! The registry is the central dispatch point: given a [`ProcGenSpec`], it
//! resolves the generator by name, creates a [`SeededRng`], and delegates
//! to [`Generator::generate()`].

use std::collections::HashMap;

use crate::generator::Generator;
use crate::output::GeneratorOutput;
use crate::rng::SeededRng;
use crate::spec::ProcGenSpec;
use crate::{ProcGenError, Result};

/// A registry of procedural generators, keyed by type name.
pub struct GeneratorRegistry {
    generators: HashMap<String, Box<dyn Generator>>,
}

impl GeneratorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            generators: HashMap::new(),
        }
    }

    /// Register a generator. Replaces any existing generator with the same
    /// type name.
    pub fn register(&mut self, generator: Box<dyn Generator>) {
        let name = generator.type_name().to_string();
        self.generators.insert(name, generator);
    }

    /// Look up a generator by type name.
    pub fn get(&self, type_name: &str) -> Option<&dyn Generator> {
        self.generators.get(type_name).map(|g| g.as_ref())
    }

    /// All registered generator type names.
    pub fn generator_names(&self) -> Vec<&str> {
        self.generators.keys().map(|k| k.as_str()).collect()
    }

    /// Resolve the generator from a spec, create a seeded RNG, and run
    /// generation.
    pub fn generate_from_spec(&self, spec: &ProcGenSpec) -> Result<GeneratorOutput> {
        let generator = self
            .generators
            .get(&spec.generator)
            .ok_or_else(|| ProcGenError::UnknownGenerator(spec.generator.clone()))?;

        let seed = spec.resolve_seed()?;
        let mut rng = SeededRng::new(seed);

        generator.generate(spec, &mut rng)
    }
}

impl Default for GeneratorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockGenerator;
    use crate::output::OutputKind;

    fn make_registry() -> GeneratorRegistry {
        let mut reg = GeneratorRegistry::new();
        reg.register(Box::new(MockGenerator));
        reg
    }

    fn mock_spec() -> ProcGenSpec {
        ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#,
        )
        .unwrap()
    }

    #[test]
    fn register_and_get() {
        let reg = make_registry();
        assert!(reg.get("mock").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn generator_names() {
        let reg = make_registry();
        let names = reg.generator_names();
        assert!(names.contains(&"mock"));
    }

    #[test]
    fn generate_from_spec_success() {
        let reg = make_registry();
        let spec = mock_spec();
        let output = reg.generate_from_spec(&spec).unwrap();
        assert_eq!(output.kind(), OutputKind::Mesh);

        let mesh = output.as_mesh().unwrap();
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.triangle_count(), 12);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn generate_from_spec_unknown_generator() {
        let reg = make_registry();
        let mut spec = mock_spec();
        spec.generator = "nonexistent".into();
        let err = reg.generate_from_spec(&spec).unwrap_err();
        assert!(matches!(err, ProcGenError::UnknownGenerator(ref name) if name == "nonexistent"));
    }

    #[test]
    fn generate_from_spec_deterministic() {
        let reg = make_registry();
        let spec = mock_spec();
        let a = reg.generate_from_spec(&spec).unwrap();
        let b = reg.generate_from_spec(&spec).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn box_dyn_generator_compiles() {
        // Object safety: this line compiling proves the trait is object-safe.
        let _: Box<dyn Generator> = Box::new(MockGenerator);
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = GeneratorRegistry::default();
        assert!(reg.generator_names().is_empty());
    }
}
