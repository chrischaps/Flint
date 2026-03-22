use crate::model::{ItemKind, Member, ModuleInfo, PublicItem};
use std::path::Path;
use syn::{self, Fields, FnArg, Item, ReturnType, Type, Visibility};

pub fn extract_public_items(source: &str) -> Vec<PublicItem> {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let mut items = Vec::new();
    for item in &file.items {
        match item {
            Item::Struct(s) if matches!(s.vis, Visibility::Public(_)) => {
                let members = match &s.fields {
                    Fields::Named(fields) => fields
                        .named
                        .iter()
                        .map(|f| Member {
                            name: f
                                .ident
                                .as_ref()
                                .map(|i| i.to_string())
                                .unwrap_or_default(),
                            ty: type_to_string(&f.ty),
                        })
                        .collect(),
                    _ => vec![],
                };
                items.push(PublicItem {
                    kind: ItemKind::Struct,
                    name: s.ident.to_string(),
                    members,
                });
            }
            Item::Enum(e) if matches!(e.vis, Visibility::Public(_)) => {
                let members = e
                    .variants
                    .iter()
                    .map(|v| {
                        let ty = match &v.fields {
                            Fields::Unnamed(fields) => fields
                                .unnamed
                                .iter()
                                .map(|f| type_to_string(&f.ty))
                                .collect::<Vec<_>>()
                                .join(", "),
                            Fields::Named(fields) => fields
                                .named
                                .iter()
                                .map(|f| {
                                    format!(
                                        "{}: {}",
                                        f.ident.as_ref().unwrap(),
                                        type_to_string(&f.ty)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                            Fields::Unit => String::new(),
                        };
                        Member {
                            name: v.ident.to_string(),
                            ty,
                        }
                    })
                    .collect();
                items.push(PublicItem {
                    kind: ItemKind::Enum,
                    name: e.ident.to_string(),
                    members,
                });
            }
            Item::Trait(t) if matches!(t.vis, Visibility::Public(_)) => {
                let members = t
                    .items
                    .iter()
                    .filter_map(|item| {
                        if let syn::TraitItem::Fn(method) = item {
                            Some(Member {
                                name: method.sig.ident.to_string(),
                                ty: fn_signature_string(&method.sig),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                items.push(PublicItem {
                    kind: ItemKind::Trait,
                    name: t.ident.to_string(),
                    members,
                });
            }
            Item::Fn(f) if matches!(f.vis, Visibility::Public(_)) => {
                let mut members: Vec<Member> = f
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|arg| {
                        if let FnArg::Typed(pat_type) = arg {
                            let name = pat_to_string(&pat_type.pat);
                            Some(Member {
                                name,
                                ty: type_to_string(&pat_type.ty),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                if let ReturnType::Type(_, ty) = &f.sig.output {
                    members.push(Member {
                        name: "return".to_string(),
                        ty: type_to_string(ty),
                    });
                }
                items.push(PublicItem {
                    kind: ItemKind::Fn,
                    name: f.sig.ident.to_string(),
                    members,
                });
            }
            _ => {}
        }
    }
    items
}

fn type_to_string(ty: &Type) -> String {
    quote::quote!(#ty)
        .to_string()
        .replace(" ", "")
        .replace(",", ", ")
}

fn pat_to_string(pat: &syn::Pat) -> String {
    quote::quote!(#pat).to_string()
}

fn fn_signature_string(sig: &syn::Signature) -> String {
    let params: Vec<String> = sig.inputs.iter().map(|arg| quote::quote!(#arg).to_string()).collect();
    let ret = match &sig.output {
        ReturnType::Default => String::new(),
        ReturnType::Type(_, ty) => format!(" -> {}", type_to_string(ty)),
    };
    format!("({}){}", params.join(", "), ret)
}

pub fn parse_crate_sources(crate_dir: &Path) -> Vec<ModuleInfo> {
    let src_dir = crate_dir.join("src");
    if !src_dir.exists() {
        return vec![];
    }

    let entry = if src_dir.join("lib.rs").exists() {
        src_dir.join("lib.rs")
    } else if src_dir.join("main.rs").exists() {
        src_dir.join("main.rs")
    } else {
        return vec![];
    };

    let entry_name = entry.file_stem().unwrap().to_string_lossy().to_string();
    vec![parse_module_file(&entry, &entry_name, &src_dir)]
}

fn parse_module_file(path: &Path, name: &str, src_dir: &Path) -> ModuleInfo {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let lines = source.lines().count();
    let public_items = extract_public_items(&source);
    let children = find_child_modules(&source, path, src_dir);
    let rel_path = path
        .strip_prefix(src_dir.parent().unwrap_or(src_dir))
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    ModuleInfo {
        name: name.to_string(),
        path: rel_path,
        lines,
        public_items,
        children,
    }
}

fn find_child_modules(source: &str, parent_path: &Path, src_dir: &Path) -> Vec<ModuleInfo> {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let parent_dir = if parent_path
        .file_name()
        .map(|f| f == "mod.rs" || f == "lib.rs" || f == "main.rs")
        .unwrap_or(false)
    {
        parent_path.parent().unwrap().to_path_buf()
    } else {
        let stem = parent_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        parent_path.parent().unwrap().join(&stem)
    };

    let mut children = Vec::new();
    for item in &file.items {
        if let Item::Mod(m) = item {
            if m.content.is_none() {
                let mod_name = m.ident.to_string();
                let mod_file = parent_dir.join(format!("{}.rs", mod_name));
                let mod_dir_file = parent_dir.join(&mod_name).join("mod.rs");

                let resolved = if mod_file.exists() {
                    Some(mod_file)
                } else if mod_dir_file.exists() {
                    Some(mod_dir_file)
                } else {
                    None
                };

                if let Some(path) = resolved {
                    children.push(parse_module_file(&path, &mod_name, src_dir));
                }
            }
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_workspace_root_for_test() -> std::path::PathBuf {
        let mut dir = std::env::current_dir().unwrap();
        loop {
            if dir.join("Cargo.toml").exists() {
                let content = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
                if content.contains("[workspace]") {
                    return dir;
                }
            }
            if !dir.pop() {
                panic!("Could not find workspace root");
            }
        }
    }

    #[test]
    fn test_parse_crate_sources_finds_modules() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-core");
        let modules = parse_crate_sources(&crate_dir);
        assert!(!modules.is_empty(), "Expected at least one module");
        assert!(
            modules.iter().any(|m| m.name == "lib"),
            "Expected 'lib' module"
        );
    }

    #[test]
    fn test_extracts_public_structs() {
        let source = r#"
            pub struct Foo {
                pub x: f32,
                pub y: f32,
            }

            struct Private {
                z: i32,
            }

            pub fn bar() {}
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 2, "Expected 2 public items, got {:?}", items);

        let foo = items.iter().find(|i| i.name == "Foo").expect("No Foo found");
        assert!(matches!(foo.kind, ItemKind::Struct));
        assert_eq!(foo.members.len(), 2);
        assert_eq!(foo.members[0].name, "x");
        assert_eq!(foo.members[0].ty, "f32");
        assert_eq!(foo.members[1].name, "y");
        assert_eq!(foo.members[1].ty, "f32");
    }

    #[test]
    fn test_extracts_enum_variants() {
        let source = r#"
            pub enum Color {
                Red,
                Green,
                Custom(u8, u8, u8),
            }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let color = &items[0];
        assert!(matches!(color.kind, ItemKind::Enum));
        assert_eq!(color.members.len(), 3);
        assert_eq!(color.members[0].name, "Red");
        assert_eq!(color.members[1].name, "Green");
        assert_eq!(color.members[2].name, "Custom");
        assert_eq!(color.members[2].ty, "u8, u8, u8");
    }

    #[test]
    fn test_extracts_trait_methods() {
        let source = r#"
            pub trait Drawable {
                fn draw(&self);
                fn bounds(&self) -> Rect;
            }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let drawable = &items[0];
        assert!(matches!(drawable.kind, ItemKind::Trait));
        assert_eq!(drawable.members.len(), 2);
        assert_eq!(drawable.members[0].name, "draw");
        assert_eq!(drawable.members[1].name, "bounds");
    }

    #[test]
    fn test_extracts_function_params() {
        let source = r#"
            pub fn process(input: &str, count: usize) -> Result<Vec<u8>, Error> {}
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let func = &items[0];
        assert!(matches!(func.kind, ItemKind::Fn));
        assert_eq!(func.members.len(), 3); // 2 params + return
        assert_eq!(func.members[0].name, "input");
        assert_eq!(func.members[0].ty, "&str");
        assert_eq!(func.members[1].name, "count");
        assert_eq!(func.members[1].ty, "usize");
        assert_eq!(func.members[2].name, "return");
        assert!(
            func.members[2].ty.contains("Result"),
            "Return type should contain 'Result', got: {}",
            func.members[2].ty
        );
    }
}
