//! The run's typed resources (ADR 0137 decision 2): one value per Rust type,
//! which every context lends, so one package lends state to another without
//! the kernel naming either.
//!
//! - **The type is the contract.** procgen's `ProcGenResolver` is inserted
//!   by whoever runs procgen; living names that type because its crate
//!   depends on flint-procgen. The kernel names neither.
//! - **A missing resource means the package isn't in the build.** The answer
//!   is `None`, where a context used to have a `#[cfg(feature = ..)]` field:
//!   rhythm asks for the debug panels and adds none when they aren't there.
//! - **Scene-scoped state stays in the subsystem** and is dropped with it.
//!   A resource is what outlives a scene (the props' pose cache, the
//!   resolver's specs) and what one package lets another add to.
//!
//! The map is keyed by `TypeId` through a pass-through hasher, since a
//! `TypeId` is already a hash: a lookup measured 1.5 ns against 7.0 ns with
//! std's SipHash (ADR 0137, Measured).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// One value per Rust type, for the run.
#[derive(Default)]
pub struct Resources {
    map: HashMap<TypeId, Box<dyn Any>, BuildHasherDefault<TypeIdHasher>>,
}

impl Resources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `value` in, returning the one of its type it replaces.
    pub fn insert<T: Any>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .map(|old| *old.downcast::<T>().expect("keyed by its own type"))
    }

    /// Take the value of type `T` out.
    pub fn remove<T: Any>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .map(|old| *old.downcast::<T>().expect("keyed by its own type"))
    }

    pub fn contains<T: Any>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    pub fn get<T: Any>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }

    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|value| value.downcast_mut::<T>())
    }

    /// The value of type `T`, put in with `make` if there is none.
    pub fn get_or_insert_with<T: Any>(&mut self, make: impl FnOnce() -> T) -> &mut T {
        self.map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(make()))
            .downcast_mut::<T>()
            .expect("keyed by its own type")
    }

    /// Two at once (living's props and procgen's resolver), on
    /// `HashMap::get_disjoint_mut`; `None` if either is missing, or if `A`
    /// and `B` are one type.
    pub fn get_two_mut<A: Any, B: Any>(&mut self) -> Option<(&mut A, &mut B)> {
        let (a, b) = (TypeId::of::<A>(), TypeId::of::<B>());
        if a == b {
            return None;
        }
        match self.map.get_disjoint_mut([&a, &b]) {
            [Some(a), Some(b)] => Some((a.downcast_mut::<A>()?, b.downcast_mut::<B>()?)),
            _ => None,
        }
    }

    /// How many values are here.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl std::fmt::Debug for Resources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resources")
            .field("len", &self.map.len())
            .finish()
    }
}

/// Passes a `TypeId`'s own hash through. `TypeId` writes its hash as one
/// `u64`; any other write folds its bytes in (FNV-1a), so the hasher is
/// right for any key, only fast for this one.
#[derive(Default)]
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write_u64(&mut self, n: u64) {
        self.0 ^= n;
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Props(u32);
    #[derive(Debug, PartialEq)]
    struct Resolver(&'static str);

    #[test]
    fn one_value_a_type_and_none_for_a_type_never_put_in() {
        let mut r = Resources::new();
        assert!(r.get::<Props>().is_none());
        assert_eq!(r.insert(Props(1)), None);
        assert_eq!(r.insert(Props(2)), Some(Props(1)), "replaces its own type");
        r.insert(Resolver("specs"));
        assert_eq!(r.len(), 2);
        r.get_mut::<Props>().unwrap().0 += 1;
        assert_eq!(r.get::<Props>(), Some(&Props(3)));
        assert_eq!(r.remove::<Resolver>(), Some(Resolver("specs")));
        assert!(!r.contains::<Resolver>());
    }

    #[test]
    fn two_at_once_lends_both_or_neither() {
        let mut r = Resources::new();
        r.insert(Props(1));
        assert!(r.get_two_mut::<Props, Resolver>().is_none(), "one missing");
        r.insert(Resolver("specs"));
        let (props, resolver) = r.get_two_mut::<Props, Resolver>().unwrap();
        props.0 = 7;
        resolver.0 = "grown";
        assert_eq!(r.get::<Props>(), Some(&Props(7)));
        assert_eq!(r.get::<Resolver>(), Some(&Resolver("grown")));
        assert!(r.get_two_mut::<Props, Props>().is_none(), "one type twice");
    }

    #[test]
    fn get_or_insert_with_makes_it_once() {
        let mut r = Resources::new();
        r.get_or_insert_with(|| Props(4)).0 += 1;
        r.get_or_insert_with(|| Props(100)).0 += 1;
        assert_eq!(r.get::<Props>(), Some(&Props(6)));
    }

    #[test]
    fn the_hasher_tells_many_types_apart() {
        // Twenty distinct types in one map: every one is found again
        macro_rules! types {
            ($($t:ident),*) => {
                $( #[derive(Debug, PartialEq)] struct $t; )*
                let mut r = Resources::new();
                $( r.insert($t); )*
                $( assert_eq!(r.get::<$t>(), Some(&$t)); )*
                assert_eq!(r.len(), [$(stringify!($t)),*].len());
            };
        }
        types!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T);
    }
}
