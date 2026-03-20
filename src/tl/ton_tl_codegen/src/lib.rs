/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![deny(unused_extern_crates)]
#![recursion_limit = "128"]

use parser::{Constructor, Delimiter, Field, Item, Matched, NameChunks, Type};
use proc_macro2::{Ident, Punct, Spacing, Span, TokenStream};
use quote::{quote, TokenStreamExt};
use serde_derive::Deserialize;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::OpenOptions,
    io::Write,
    iter::{repeat, FromIterator},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
};

pub mod parser {
    use pom::{
        self,
        char_class::{alphanum, digit, hex_digit},
        parser::*,
        Parser,
    };

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Type {
        Int,
        Flags,
        Named(Vec<String>),
        TypeParameter(String),
        Generic(Vec<String>, Box<Type>),
        Flagged(String, u32, Box<Type>),
        Repeated(Option<u32>, Vec<Field>),
    }

    impl Type {
        fn get_dotted_name(dotted_ident: &[String]) -> String {
            dotted_ident.join(".")
        }

        fn get_generic_name(dotted_ident: &[String], ty: &Type) -> String {
            Self::get_dotted_name(dotted_ident) + " " + ty.get_name().as_str()
        }

        fn get_flagged_name(name: &str, bit: u32, ty: &Type) -> String {
            name.to_owned() + "." + bit.to_string().as_str() + "?" + ty.get_name().as_str()
        }

        fn get_repeated_name(repeat_count: Option<u32>, fields: &[Field]) -> String {
            let mut result = String::new();
            if let Some(value) = repeat_count {
                result = format!("{} * ", value);
            }
            result += "[ ";
            for field in fields {
                result += &(field.get_str() + " ");
            }
            result + "]"
        }

        pub fn get_name(&self) -> String {
            match self {
                Type::Int => "#".to_owned(),
                Type::Flags => "flags".to_owned(),
                Type::Named(v) => Self::get_dotted_name(v),
                Type::TypeParameter(ty) => ty.clone(),
                Type::Generic(v, ty) => Self::get_generic_name(v, ty),
                Type::Flagged(name, bit, ty) => Self::get_flagged_name(name, *bit, ty),
                Type::Repeated(repeat_count, fields) => {
                    Self::get_repeated_name(*repeat_count, fields)
                }
            }
        }

        pub fn names_vec(&self) -> Option<&[String]> {
            match self {
                Type::Int
                | Type::Flags
                | Type::TypeParameter(..)
                | Type::Flagged(..)
                | Type::Repeated(..) => None,
                Type::Named(v) | Type::Generic(v, ..) => Some(v),
            }
        }

        pub fn names_vec_mut(&mut self) -> Option<&mut Vec<String>> {
            match *self {
                Type::Int
                | Type::Flags
                | Type::TypeParameter(..)
                | Type::Flagged(..)
                | Type::Repeated(..) => None,
                Type::Named(ref mut v) | Type::Generic(ref mut v, ..) => Some(v),
            }
        }

        pub fn owned_names_vec(&self) -> Vec<String> {
            match self.names_vec() {
                Some(vec) => vec.to_vec(),
                None => Vec::new(),
            }
        }

        pub fn namespaces(&self) -> &[String] {
            self.names_vec().map(|v| &v[..(v.len() - 1).max(0)]).unwrap_or(&[])
        }

        pub fn name(&self) -> Option<&str> {
            self.names_vec().and_then(|v| v.last().map(String::as_str))
        }

        pub fn flag_field(&self) -> Option<(String, u32)> {
            match self {
                Type::Flagged(f, b, _) => Some((f.clone(), *b)),
                _ => None,
            }
        }

        pub fn is_type_parameter(&self) -> bool {
            matches!(self, Type::TypeParameter(..))
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Field {
        pub name: Option<String>,
        pub ty: Type,
    }

    impl Field {
        pub fn get_str(&self) -> String {
            match self.name {
                Some(ref name) => name.clone() + ":" + self.ty.get_name().as_str(),
                None => self.ty.get_name(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Constructor<Ty, Fi> {
        pub variant: Ty,
        pub tl_id: Option<u32>,
        pub type_parameters: Vec<Fi>,
        pub fields: Vec<Fi>,
        pub output: Ty,
        pub original_variant: String,
        pub original_output: String,
        pub is_function: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Delimiter {
        Types,
        Functions,
    }

    #[derive(Debug, Clone)]
    pub enum Item {
        Delimiter(Delimiter),
        Constructor(Constructor<Type, Field>),
    }

    #[derive(Debug, Clone)]
    pub struct Matched<T>(pub T, pub String);

    impl Eq for Matched<Constructor<Type, Field>> {}

    impl PartialEq for Matched<Constructor<Type, Field>> {
        fn eq(&self, other: &Self) -> bool {
            PartialEq::eq(&self.1, &other.1)
        }
    }

    impl Ord for Matched<Constructor<Type, Field>> {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            Ord::cmp(&self.1, &other.1)
        }
    }

    impl PartialOrd for Matched<Constructor<Type, Field>> {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            PartialOrd::partial_cmp(&self.1, &other.1)
        }
    }

    fn utf8(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    fn ident() -> Parser<u8, String> {
        (is_a(alphanum) | sym(b'_')).repeat(1..).map(utf8)
    }

    fn dotted_ident() -> Parser<u8, Vec<String>> {
        ((ident() - sym(b'.')).repeat(0..) + ident()).map(|(mut v, i)| {
            v.push(i);
            v
        })
    }

    fn tl_id() -> Parser<u8, u32> {
        sym(b'#') * is_a(hex_digit).repeat(0..9).convert(|s| u32::from_str_radix(&utf8(s), 16))
    }

    fn decimal() -> Parser<u8, u32> {
        is_a(digit).repeat(0..).convert(|s| utf8(s).parse())
    }

    fn ty_flag() -> Parser<u8, Type> {
        (ident() - sym(b'.') + decimal() - sym(b'?') + call(ty))
            .map(|((name, bit), ty)| Type::Flagged(name, bit, Box::new(ty)))
    }

    fn ty_generic() -> Parser<u8, Type> {
        (sym(b'(') * dotted_ident() - space() + call(ty) - sym(b')'))
            .map(|(name, ty)| Type::Generic(name, Box::new(ty)))
            | (dotted_ident() - sym(b'<') + call(ty) - sym(b'>'))
                .map(|(name, ty)| Type::Generic(name, Box::new(ty)))
    }

    fn ty() -> Parser<u8, Type> {
        sym(b'#').map(|_| Type::Int)
            | (sym(b'!') * ident().map(Type::TypeParameter))
            | ty_flag()
            | ty_generic()
            | dotted_ident().map(Type::Named)
    }

    fn ty_space_generic() -> Parser<u8, Type> {
        let space_generic = dotted_ident() - space() + ty();
        space_generic.map(|(name, ty)| Type::Generic(name, Box::new(ty))) | ty()
    }

    fn base_field() -> Parser<u8, Field> {
        (ident() - sym(b':') + ty()).map(|(name, ty)| Field { name: Some(name), ty }).name("field")
    }

    fn repeated_field() -> Parser<u8, Field> {
        ((decimal() - space() * sym(b'*') * space()).opt() + sym(b'[') * call(base_fields)
            - seq(b" ]"))
        .map(|(repeat_count, fv)| Field { name: None, ty: Type::Repeated(repeat_count, fv) })
    }

    fn base_field_anonymous_or_repeated() -> Parser<u8, Field> {
        repeated_field() | base_field() | ty().map(|ty| Field { name: None, ty })
    }

    fn base_fields() -> Parser<u8, Vec<Field>> {
        (space() * base_field_anonymous_or_repeated()).repeat(0..)
    }

    fn ty_param_field() -> Parser<u8, Field> {
        sym(b'{') * base_field() - sym(b'}')
    }

    fn fields() -> Parser<u8, (Vec<Field>, Vec<Field>)> {
        (space().opt() * sym(b'?')).map(|_| (vec![], vec![]))
            | ((space() * ty_param_field()).repeat(0..) + base_fields())
    }

    fn output_and_matched<T: 'static>(inner: Parser<u8, T>) -> Parser<u8, Matched<T>> {
        Parser::new(move |input| {
            let start = input.position();
            let output = inner.parse(input)?;
            let end = input.position();
            Ok(Matched(output, utf8(input.segment(start, end))))
        })
    }

    fn get_tl_id(
        parsed_id: Option<u32>,
        variant: &str,
        type_parameters: &Vec<Field>,
        fields: &Vec<Field>,
        output: &str,
    ) -> Option<u32> {
        match parsed_id {
            Some(id) => Some(id),
            None => {
                let mut string = variant.to_string();
                for ty in type_parameters {
                    string += &(" {".to_owned() + ty.get_str().as_str() + "}");
                }
                for field in fields {
                    string += " ";
                    string += field.get_str().as_str();
                }
                string += " = ";
                string += output;
                Some(crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(string.as_bytes()))
            }
        }
    }

    fn constructor() -> Parser<u8, Constructor<Type, Field>> {
        (output_and_matched(dotted_ident()) + tl_id().opt() + fields()
            - space()
            - sym(b'=')
            - space()
            + output_and_matched(ty_space_generic())
            - sym(b';'))
        .map(|(((variant, tl_id), (type_parameters, fields)), output)| Constructor {
            tl_id: get_tl_id(tl_id, &variant.1, &type_parameters, &fields, &output.1),
            type_parameters,
            fields,
            original_variant: variant.1,
            variant: Type::Named(variant.0),
            original_output: output.1,
            output: output.0,
            is_function: false,
        })
        .name("constructor")
    }

    fn delimiter() -> Parser<u8, Delimiter> {
        seq(b"---types---").map(|_| Delimiter::Types)
            | seq(b"---functions---").map(|_| Delimiter::Functions)
    }

    fn space() -> Parser<u8, ()> {
        let end_comment = || seq(b"*/");
        (one_of(b" \t\r\n").discard()
            | (seq(b"//") - none_of(b"\r\n").repeat(0..)).discard()
            | (seq(b"/*") * (!end_comment() * take(1)).repeat(0..) * end_comment()).discard())
        .repeat(0..)
        .discard()
    }

    fn item() -> Parser<u8, Matched<Item>> {
        output_and_matched({
            delimiter().map(Item::Delimiter) | constructor().map(Item::Constructor)
        }) - space()
    }

    fn lines() -> Parser<u8, Vec<Matched<Item>>> {
        space() * item().repeat(0..) - end()
    }

    pub fn parse_string(input: &str) -> Result<Vec<Matched<Item>>, pom::Error> {
        let parser = lines();
        let mut ret = Vec::new();
        let mut sentence = String::new();
        for line in input.lines() {
            sentence.push_str(line);
            if !line.ends_with(';') {
                sentence.push('\n');
                continue;
            }
            let mut input = pom::DataInput::new(sentence.as_bytes());
            match parser.parse(&mut input) {
                Err(e) => {
                    println!("Error when parsing line: {}", sentence);
                    return Err(e);
                }
                Ok(mut items) => {
                    ret.append(&mut items);
                    sentence.clear();
                }
            }
        }
        Ok(ret)
    }

    fn name_ident() -> Parser<u8, NameChunks> {
        ident().map(|s| NameChunks(vec![s]))
    }

    #[derive(Debug, Clone)]
    pub struct NameChunks(pub Vec<String>);

    impl NameChunks {
        pub fn from_name(name: &str) -> Result<Self, pom::Error> {
            let mut input = pom::DataInput::new(name.as_bytes());
            (name_ident() - end()).parse(&mut input)
        }

        pub fn as_snake_case(&self) -> String {
            let names: Vec<String> = self.0.iter().map(|s| s.to_ascii_lowercase()).collect();
            names.join("_")
        }

        pub fn as_upper_camel_case(&self) -> String {
            self.0
                .iter()
                .cloned()
                .map(|mut s| {
                    s[..1].make_ascii_uppercase();
                    s
                })
                .collect()
        }

        pub fn common_prefix_of(&self, other: &Self) -> Self {
            let index = self
                .0
                .iter()
                .zip(&other.0)
                .enumerate()
                .find(|&(_, (a, b))| a != b)
                .map_or_else(|| self.0.len().min(other.0.len()), |(e, _)| e);
            NameChunks(self.0[..index].to_vec())
        }

        pub fn trim_common_prefix(&mut self, other: &Self) {
            assert!(self.0.starts_with(&other.0), "{:?} not a prefix of {:?}", self, other);
            self.0.drain(0..(other.0.len()));
        }
    }
}

fn fail_hard() -> TokenStream {
    quote!(FAIL_LOUDLY_AT_COMPILE_TIME!())
}

#[derive(Debug)]
struct Constructors<Ty, Fi>(Vec<Matched<Constructor<Ty, Fi>>>);

impl<Ty, Fi> Default for Constructors<Ty, Fi> {
    fn default() -> Self {
        Constructors(Vec::new())
    }
}

type TypeResolutionMap = BTreeMap<Vec<String>, TypeIR>;

#[derive(Debug)]
enum NamespaceItem {
    AsEnum(Constructors<TypeIR, FieldIR>),
    AsVariant(Matched<Constructor<TypeIR, FieldIR>>),
    AsFunction(Matched<Constructor<TypeIR, FieldIR>>),
    AnotherNamespace(Namespace),
}

fn write_to_file(contents: impl ToString, filename: &Path, append: bool) {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(!append).append(append);

    let mut file = options.open(filename).unwrap_or_else(|err| {
        panic!(
            "Unable to open file <{filename}> with the given parameters: {options:?}: {err}",
            filename = filename.to_string_lossy(),
            options = options,
            err = err
        )
    });

    file.write_all(contents.to_string().as_bytes()).unwrap_or_else(|err| {
        panic!("Unable to write contents into the file: {}: {}", filename.to_string_lossy(), err)
    });
}

fn reformat(filename: &Path) {
    static WARNING_PRINTED: AtomicBool = AtomicBool::new(false);
    if !cfg!(feature = "reformat") {
        if !WARNING_PRINTED.swap(true, Ordering::Relaxed) {
            println!(
                "use feature \"reformat\" in ton_api cargo.toml for ton_tl_codegen \
                to get formatted rs files, but it slows down generation process. \
                You can reformat manualy desired files (Shift + Alt + F in VSCode)."
            );
        }
        return;
    }
    const INSTALL_INSTRUCTIONS: &str = "It's not an issue, the building will proceed. \
        If you wish to develop using ton_api, you can install rustfmt by running command: \
        `rustup component add rustfmt`";
    let status = match Command::new("rustfmt").arg("--edition").arg("2018").arg(filename).status() {
        Ok(status) => status,
        Err(err) => {
            if !WARNING_PRINTED.swap(true, Ordering::Relaxed) {
                println!(
                    "cargo:warning=rustfmt failed to start: {:?}. {}",
                    err, INSTALL_INSTRUCTIONS
                );
            }
            return;
        }
    };
    #[allow(clippy::collapsible_if)]
    if !status.success() {
        if !WARNING_PRINTED.swap(true, Ordering::Relaxed) {
            println!(
                "cargo:warning=rustfmt command returned code: {}. {}",
                status.code().unwrap(),
                INSTALL_INSTRUCTIONS
            );
        }
    }
}

#[derive(Debug, Default)]
struct Namespace(BTreeMap<syn::Ident, NamespaceItem>);

impl Namespace {
    fn descend_tree(&mut self, names: &[syn::Ident]) -> &mut Self {
        names.iter().fold(self, |ns, name| {
            match ns
                .0
                .entry(name.clone())
                .or_insert_with(|| NamespaceItem::AnotherNamespace(Default::default()))
            {
                &mut NamespaceItem::AnotherNamespace(ref mut ns) => ns,
                other => panic!(
                    "descend_tree: duplicate namespace item {} {:?} {:?}",
                    name, other, names
                ),
            }
        })
    }

    fn insert(&mut self, mut names: Vec<syn::Ident>, item: NamespaceItem) {
        let leaf = names.pop().unwrap();
        let namespace = self.descend_tree(&names);
        if namespace.0.contains_key(&leaf) {
            println!("cargo:warning=insert: duplicate namespace item {:?}", names);
            return;
        }
        namespace.0.insert(leaf, item);
    }

    fn print_rust(
        &self,
        config: &Option<Config>,
        prelude: impl ToString,
        path: &Path,
        append: bool,
    ) -> (PathBuf, PathBuf) {
        let mut has_submodules = false;
        let items = self.0.iter().map(|(name, item)| {
            match item {
                NamespaceItem::AsEnum(cs) => cs.as_enum(config),
                NamespaceItem::AsVariant(cm) => cm.0.as_variant_type_struct(config, &cm.1),
                NamespaceItem::AsFunction(cm) => cm.0.as_function_struct(config, &cm.1),
                NamespaceItem::AnotherNamespace(ns) => {
                    let prelude = quote! {
                        // #![allow(clippy::large_enum_variant)]
                        use crate::{Constructor, ConstructorOnly};
                        use serde_derive::{Serialize, Deserialize};
                    };
                    let (filename, _dir) = ns.print_rust(
                        config,
                        prelude,
                        path.join(name.to_string()).as_path(),
                        false,
                    );
                    reformat(&filename);
                    has_submodules = true;

                    quote! { pub mod #name; }
                }
            }
        });

        let contents = quote!(#( #items )*);

        let filename = if has_submodules { path.join("mod.rs") } else { path.with_extension("rs") };

        let dir = filename
            .parent()
            .unwrap_or_else(|| {
                panic!("Unable to get parent directory for: {}", filename.to_string_lossy())
            })
            .to_path_buf();

        std::fs::create_dir_all(&dir).unwrap_or_else(|err| {
            panic!("Unable to create directory: {} err: {}", filename.to_string_lossy(), err)
        });

        write_to_file(prelude, &filename, append);
        write_to_file(contents, &filename, true);
        (filename, dir)
    }

    fn populate_all_constructors<'this>(
        &'this self,
        to_populate: &mut Vec<&'this Constructor<TypeIR, FieldIR>>,
    ) {
        for item in self.0.values() {
            match item {
                NamespaceItem::AsEnum(cs) => {
                    to_populate.extend(cs.0.iter().map(|cm| &cm.0));
                }
                NamespaceItem::AsFunction(c) => {
                    to_populate.push(&c.0);
                }
                NamespaceItem::AnotherNamespace(ns) => {
                    ns.populate_all_constructors(to_populate);
                }
                _ => {}
            };
        }
    }
}

#[derive(Deserialize)]
pub struct Config {
    exclude_types: HashSet<String>,
    need_box: HashSet<String>,
    need_determiner: HashSet<String>,
    replace_with_bytes: HashSet<String>,
    additional_derives: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
struct AllConstructors {
    items: Namespace,
}

fn filter_items(config: &Option<Config>, iv: &mut Vec<Matched<Item>>) {
    let built_in_types: HashSet<&'static str> = [
        "int",
        "int32",
        "int53",
        "int64",
        "int128",
        "int256",
        "int512",
        "long",
        "double",
        "bytes",
        "vector",
        "string",
        "object",
        "function",
        "Object",
        "Function",
        "secureString",
        "secureBytes",
        "true",
        "false",
    ]
    .iter()
    .cloned()
    .collect();

    iv.retain(|Matched(ref i, _)| {
        let c = match i {
            Item::Constructor(c) => c,
            _ => return true,
        };
        // Blacklist some annoying inconsistencies.
        if built_in_types.contains(&c.original_variant.as_str()) {
            return false;
        }
        if let Some(config) = config {
            return !config.exclude_types.contains(&c.original_variant);
        }
        true
    });
}

impl AllConstructors {
    fn from_matched_items(config: &Option<Config>, iv: Vec<Matched<Item>>) -> Self {
        let mut current = Delimiter::Types;
        let mut ret = AllConstructors { items: Default::default() };
        let mut constructors_tree: BTreeMap<Vec<String>, Constructors<Type, Field>> =
            BTreeMap::new();
        let mut functions: Vec<Matched<Constructor<Type, Field>>> = Vec::new();
        for Matched(item, text) in iv {
            match item {
                Item::Delimiter(delimiter) => current = delimiter,
                Item::Constructor(mut constructor) => match current {
                    Delimiter::Types => {
                        let vec = &mut constructors_tree
                            .entry(constructor.output.owned_names_vec())
                            .or_default()
                            .0;
                        let cons = Matched(constructor, text);
                        if let Err(index) = vec.binary_search(&cons) {
                            vec.insert(index, cons);
                        }
                    }
                    Delimiter::Functions => {
                        constructor.is_function = true;
                        functions.push(Matched(constructor, text));
                    }
                },
            }
        }
        let mut resolve_map: TypeResolutionMap = Default::default();
        for cs in constructors_tree.values_mut() {
            let base_ns = cs.first_constructor().output.namespaces().to_vec();
            cs.fix_names(config, &base_ns, &mut resolve_map);
        }
        for &mut Matched(ref mut c, _) in &mut functions {
            camelize(config, &mut resolve_map, &mut c.variant, |_, ns| {
                ns.insert(0, "rpc".to_string());
                false
            });
        }
        for (_, cs) in constructors_tree {
            let cs = cs.resolve(config, &resolve_map);
            for Matched(c, m) in &cs.0 {
                ret.items.insert(
                    c.variant.owned_names_vec(),
                    NamespaceItem::AsVariant(Matched(c.clone(), m.clone())),
                );
            }
            ret.items
                .insert(cs.first_constructor().output.owned_names_vec(), NamespaceItem::AsEnum(cs));
        }
        for Matched(c, m) in functions {
            let c = c.resolve(config, Delimiter::Functions, &resolve_map);
            ret.items.insert(c.variant.owned_names_vec(), NamespaceItem::AsFunction(Matched(c, m)));
        }
        ret
    }

    fn as_lazy_statics(&self) -> TokenStream {
        let mut all_constructors = Default::default();
        self.items.populate_all_constructors(&mut all_constructors);
        let dynamic_deserializers =
            all_constructors.iter().filter_map(|c| c.as_dynamic_deserializer());

        quote! {
            fn make_deserializers() -> Vec<crate::DynamicDeserializer> {
                vec![ #( #dynamic_deserializers ),* ]
            }

            lazy_static::lazy_static! {
                static ref ALL_DESERIALIZERS: Vec<crate::DynamicDeserializer> = make_deserializers();

                pub static ref BY_NUMBER:
                std::collections::BTreeMap<u32, &'static crate::DynamicDeserializer> =
                    ALL_DESERIALIZERS.iter()
                        .map(|d| (d.id, d))
                        .collect();

                pub static ref BY_NAME:
                std::collections::BTreeMap<&'static str, &'static crate::DynamicDeserializer> =
                    ALL_DESERIALIZERS.iter()
                        .map(|d| (d.type_name, d))
                        .collect();
            }
        }
    }

    fn print_tokens(&self, config: &Option<Config>, prelude: impl ToString, path: &Path) {
        let (filename, dir) = self.items.print_rust(config, prelude, path, false);
        let dynamic_deserializers = self.as_lazy_statics();
        write_to_file(dynamic_deserializers, &dir.join("dynamic.rs"), false);
        write_to_file(quote! { pub mod dynamic; }, &filename, true);
        reformat(&filename)
    }
}

fn no_conflict_ident(s: &str) -> syn::Ident {
    let mut candidate: String = s.into();
    loop {
        match syn::parse_str(&candidate) {
            Ok(i) => return i,
            Err(_) => candidate.push('_'),
        }
    }
}

fn no_conflict_local_ident(s: &str) -> Option<syn::Ident> {
    match s {
        "bytes" => Some(syn::parse_str("bytes_").unwrap()),
        _ => None,
    }
}

fn wrap_option_type(wrap: bool, ty: TokenStream) -> TokenStream {
    if wrap {
        quote! { Option<#ty> }
    } else {
        ty
    }
}

fn wrap_option_value(wrap: bool, ty: TokenStream) -> TokenStream {
    if wrap {
        quote! { Some(#ty) }
    } else {
        ty
    }
}

#[derive(Debug, Clone)]
struct TypeName {
    tokens: TokenStream,
    tokens_canon: String,
    idents: Option<Vec<syn::Ident>>,
}

impl PartialEq for TypeName {
    fn eq(&self, other: &Self) -> bool {
        self.tokens_canon == other.tokens_canon
    }
}

impl Eq for TypeName {}

impl PartialOrd for TypeName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TypeName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.tokens_canon.cmp(&other.tokens_canon)
    }
}

impl TypeName {
    fn transformed_tokens<F>(&self, func: F) -> TokenStream
    where
        F: FnOnce(&TokenStream) -> TokenStream,
    {
        func(&self.tokens)
    }

    fn transformed<F>(&self, func: F) -> Self
    where
        F: FnOnce(&TokenStream) -> TokenStream,
    {
        let tokens = self.transformed_tokens(func);
        let tokens_canon = format!("{}", tokens);
        TypeName { tokens, tokens_canon, idents: None }
    }
}

fn to_snake_case(ident: &str) -> String {
    let mut result = String::new();
    for c in ident.chars() {
        if c.is_ascii_uppercase() && !result.is_empty() {
            result.push_str(format!("_{}", c.to_ascii_lowercase()).as_str());
        } else {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}

impl<S> FromIterator<S> for TypeName
where
    S: AsRef<str>,
{
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = S>,
    {
        let mut tokens = quote!(crate::ton);
        let mut idents = vec![];
        let mut iter = iter.into_iter();
        if let Some(mut last_segment) = iter.next() {
            for segment in iter {
                let ident = no_conflict_ident(to_snake_case(last_segment.as_ref()).as_str());
                idents.push(ident);
                last_segment = segment;
            }
            idents.push(no_conflict_ident(last_segment.as_ref()));
        }

        for ident in &idents {
            tokens = quote!(#tokens::#ident);
        }
        let tokens_canon = format!("{}", tokens);

        TypeName { tokens, tokens_canon, idents: Some(idents) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum WireKind {
    Bare(TypeName),
    Boxed(TypeName),
    TypeParameter(syn::Ident),
    FlaggedTrue,
    Flags,
    ExtraDefault(TypeName),
}

fn is_first_char_lowercase(s: &str) -> bool {
    s.chars().next().map(char::is_lowercase).unwrap_or(false)
}

impl WireKind {
    fn from_names_and_hint(names: &[String], force_bare: bool) -> Self {
        match names.last().map(String::as_str) {
            Some("true") => WireKind::FlaggedTrue,
            Some("function") | Some("object") => {
                WireKind::Boxed(names.iter().map(String::as_str).collect())
            }
            Some(s) if force_bare || is_first_char_lowercase(s) => {
                WireKind::Bare(names.iter().map(String::as_str).collect())
            }
            Some(_) => WireKind::Boxed(names.iter().map(String::as_str).collect()),
            None => unimplemented!(),
        }
    }

    fn type_parameter(id: syn::Ident) -> Self {
        WireKind::TypeParameter(id)
    }

    fn become_container_for(&mut self, include_determiner: bool, contained: Self) {
        let ty_loc = match self {
            WireKind::Bare(ref mut t) | WireKind::Boxed(ref mut t) => t,
            _ => unimplemented!(),
        };
        let contained = if include_determiner {
            match contained {
                WireKind::Bare(ty) | WireKind::Boxed(ty) => ty.tokens,
                WireKind::TypeParameter(t) => quote!(#t), //quote!(crate::ton::Boxed, #t),
                _ => unimplemented!(),
            }
        } else {
            match contained {
                WireKind::Bare(t) | WireKind::Boxed(t) => t.tokens,
                WireKind::TypeParameter(t) => quote!(#t),
                _ => unimplemented!(),
            }
        };
        *ty_loc = ty_loc.transformed(|ty| quote!(#ty<#contained>));
    }

    fn as_read_method(&self) -> TokenStream {
        match *self {
            WireKind::Bare(..) | WireKind::Flags => quote!(read_bare),
            WireKind::Boxed(..) | WireKind::TypeParameter(..) => quote!(read_boxed),
            WireKind::ExtraDefault(..) => quote!(just_default),
            WireKind::FlaggedTrue => fail_hard(),
        }
    }

    fn as_write_method(&self) -> Option<TokenStream> {
        match *self {
            WireKind::Bare(..) | WireKind::Flags => Some(quote!(write_bare)),
            WireKind::Boxed(..) | WireKind::TypeParameter(..) => Some(quote!(write_boxed)),
            WireKind::ExtraDefault(..) => None,
            WireKind::FlaggedTrue => Some(fail_hard()),
        }
    }

    fn is_unit(&self) -> bool {
        matches!(self, WireKind::FlaggedTrue)
    }

    fn is_flags(&self) -> bool {
        matches!(self, WireKind::Flags)
    }

    fn is_type_parameter(&self) -> bool {
        matches!(self, WireKind::TypeParameter(..))
    }

    fn is_extra(&self) -> bool {
        matches!(self, WireKind::ExtraDefault(..))
    }

    fn opt_names_slice(&self) -> Option<&[syn::Ident]> {
        match self {
            WireKind::Bare(t) | WireKind::Boxed(t) | WireKind::ExtraDefault(t) => {
                t.idents.as_deref()
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TypeIR {
    wire_kind: WireKind,
    needs_box: bool,
    needs_determiner: bool,
    with_option: bool,
    contained: Option<(Vec<Ident>, bool)>,
}

impl TypeIR {
    fn from_names(config: &Option<Config>, names: &[String]) -> Self {
        Self::from_names_and_hint(config, names, false)
    }

    fn from_names_and_hint(config: &Option<Config>, names: &[String], force_bare: bool) -> Self {
        let wire_kind = WireKind::from_names_and_hint(names, force_bare);
        let (needs_box, needs_determiner) = if let Some(name) = names.last() {
            let needs_determiner = name == "vector" || name == "Vector";
            if let Some(config) = config {
                (
                    if config.need_box.contains(name) {
                        true
                    } else {
                        let len = names.len();
                        if len >= 2 {
                            let name = format!("{}.{}", names[len - 2], names[len - 1]);
                            config.need_box.contains(&name)
                        } else {
                            false
                        }
                    },
                    needs_determiner || config.need_determiner.contains(name),
                )
            } else {
                (false, needs_determiner)
            }
        } else {
            (false, false)
        };

        TypeIR { wire_kind, with_option: false, needs_box, needs_determiner, contained: None }
    }

    fn bytes() -> Self {
        TypeIR {
            wire_kind: WireKind::Bare(["bytes"].iter().collect()),
            needs_box: false,
            needs_determiner: false,
            with_option: false,
            contained: None,
        }
    }

    fn int() -> Self {
        TypeIR {
            wire_kind: WireKind::Bare(["int"].iter().collect()),
            needs_box: false,
            needs_determiner: false,
            with_option: false,
            contained: None,
        }
    }

    fn type_parameter(id: syn::Ident) -> Self {
        TypeIR {
            wire_kind: WireKind::type_parameter(id),
            needs_box: false,
            needs_determiner: false,
            with_option: false,
            contained: None,
        }
    }

    fn flags() -> Self {
        TypeIR {
            wire_kind: WireKind::Flags,
            needs_box: false,
            needs_determiner: false,
            with_option: false,
            contained: None,
        }
    }

    fn repeated() -> Self {
        unimplemented!()
    }

    fn with_container(self, mut container: TypeIR) -> Self {
        let bare = matches!(&self.wire_kind, WireKind::Bare(_));
        container.contained = Some((self.owned_names_vec(), bare));
        container.wire_kind.become_container_for(container.needs_determiner, self.wire_kind);
        container
    }

    fn with_option_wrapper(mut self) -> Self {
        if !self.wire_kind.is_unit() {
            self.with_option = true;
        }
        self
    }

    fn io_turbofish(&self, with_tlobject: bool) -> TokenStream {
        let ty = match self.wire_kind {
            WireKind::Flags => quote!(crate::ton::Flags),
            WireKind::TypeParameter(_) if with_tlobject => quote!(crate::ton::TLObject),
            _ => self.non_field_type(),
        };
        quote!(::<#ty>)
    }

    fn assemble_method(&self, with_tlobject: bool, method: TokenStream) -> TokenStream {
        let turbofish = self.io_turbofish(with_tlobject);
        quote!(#method #turbofish)
    }

    fn as_read_method(&self) -> TokenStream {
        self.assemble_method(true, self.wire_kind.as_read_method())
    }

    fn as_write_method(&self) -> Option<TokenStream> {
        self.wire_kind.as_write_method().map(|m| self.assemble_method(false, m))
    }

    fn non_field_type(&self) -> TokenStream {
        match self.wire_kind {
            WireKind::Bare(ref t) | WireKind::Boxed(ref t) | WireKind::ExtraDefault(ref t) => {
                t.tokens.clone()
            }
            WireKind::TypeParameter(ref t) => quote!(#t),
            _ => fail_hard(),
        }
    }

    fn unboxed(&self) -> TokenStream {
        wrap_option_type(self.with_option, self.non_field_type())
    }

    fn boxed(&self) -> TokenStream {
        let mut ty = self.non_field_type();
        if self.needs_box {
            ty = quote!(Box<#ty>);
        }
        wrap_option_type(self.with_option, ty)
    }

    fn field_type(&self) -> TokenStream {
        if self.is_unit() {
            quote!(bool)
        } else {
            self.boxed()
        }
    }

    fn ref_prefix(&self) -> TokenStream {
        if self.is_unit() {
            quote!()
        } else {
            quote!(ref)
        }
    }

    fn reference_prefix(&self) -> TokenStream {
        if self.is_unit() {
            quote!()
        } else {
            quote!(&)
        }
    }

    fn local_reference_prefix(&self) -> TokenStream {
        if self.is_unit() {
            quote!(&)
        } else {
            quote!()
        }
    }

    fn field_reference_type(&self) -> TokenStream {
        let ref_ = self.reference_prefix();
        let mut ty = if self.is_unit() { quote!(bool) } else { self.non_field_type() };
        if self.needs_box {
            ty = quote!(Box<#ty>);
        }
        wrap_option_type(self.with_option, quote!(#ref_ #ty))
    }

    fn as_field_reference(&self, on: TokenStream) -> TokenStream {
        if self.is_unit() {
            wrap_option_value(self.with_option, quote!(#on))
        } else if self.with_option {
            quote!(#on.as_ref())
        } else {
            quote!(&#on)
        }
    }

    fn is_defined_trailer(&self) -> TokenStream {
        match self.wire_kind {
            _ if self.with_option => quote!(.is_some()),
            WireKind::FlaggedTrue => quote!(),
            _ => fail_hard(),
        }
    }

    fn is_unit(&self) -> bool {
        self.wire_kind.is_unit()
    }

    fn is_flags(&self) -> bool {
        self.wire_kind.is_flags()
    }

    fn is_type_parameter(&self) -> bool {
        self.wire_kind.is_type_parameter()
    }

    fn is_extra(&self) -> bool {
        self.wire_kind.is_extra()
    }

    fn owned_names_vec(&self) -> Vec<syn::Ident> {
        self.wire_kind.opt_names_slice().unwrap().to_vec()
    }

    fn name(&self) -> syn::Ident {
        self.wire_kind.opt_names_slice().and_then(|s| s.last()).unwrap().clone()
    }

    fn contained(&self) -> (TokenStream, bool) {
        let Some((idents, bare)) = &self.contained else { panic!("Wrong type name") };
        let mut tokens = quote!(crate::ton);
        for ident in idents {
            tokens = quote!(#tokens::#ident);
        }
        (tokens, *bare)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FieldIR {
    name: String,
    ty: TypeIR,
    flag_bit: Option<(String, u32)>,
}

impl Field {
    fn resolved(
        &self,
        config: &Option<Config>,
        resolve_map: &TypeResolutionMap,
        replace_string_with_bytes: bool,
    ) -> FieldIR {
        let ty = if replace_string_with_bytes && self.ty.name() == Some("string") {
            TypeIR::bytes()
        } else {
            let is_flag_field =
                if let Some(ref value) = self.name { value == "flags" } else { false };
            self.ty.resolved(config, resolve_map, is_flag_field)
        };

        let name = if let Some(ref value) = self.name { value.clone() } else { String::new() };
        FieldIR { ty, name, flag_bit: self.ty.flag_field() }
    }
}

impl FieldIR {
    fn name(&self) -> syn::Ident {
        no_conflict_ident(&self.name)
    }

    fn local_name(&self) -> Option<syn::Ident> {
        no_conflict_local_ident(&self.name)
    }

    fn as_field(&self) -> TokenStream {
        let name = self.name();
        let ty = self.ty.field_type();

        quote! {
            #name: #ty
        }
    }

    fn extra_default(field_name: &str, wire_kind_type: TypeName) -> Self {
        FieldIR {
            name: field_name.to_string(),
            ty: TypeIR {
                wire_kind: WireKind::ExtraDefault(wire_kind_type),
                needs_box: false,
                needs_determiner: false,
                with_option: false,
                contained: None,
            },
            flag_bit: None,
        }
    }
}

impl Type {
    fn resolved(
        &self,
        config: &Option<Config>,
        resolve_map: &TypeResolutionMap,
        is_flag_field: bool,
    ) -> TypeIR {
        match self {
            Type::Named(names) => match resolve_map.get(names) {
                Some(ir) => ir.clone(),
                None => TypeIR::from_names(config, names),
            },
            Type::TypeParameter(name) => TypeIR::type_parameter(no_conflict_ident(name)),
            Type::Generic(container, ty) => {
                let ty = ty.resolved(config, resolve_map, false);
                let container = match resolve_map.get(container) {
                    Some(ir) => ir.clone(),
                    None => TypeIR::from_names(config, container),
                };
                ty.with_container(container)
            }
            Type::Flagged(_, _, ty) => {
                ty.resolved(config, resolve_map, false).with_option_wrapper()
            }
            Type::Flags => TypeIR::flags(),
            Type::Int if is_flag_field => TypeIR::flags(),
            Type::Int => TypeIR::int(),
            Type::Repeated(..) => TypeIR::repeated(),
        }
    }
}

impl Constructor<Type, Field> {
    fn resolve(
        self,
        config: &Option<Config>,
        which: Delimiter,
        resolve_map: &TypeResolutionMap,
    ) -> Constructor<TypeIR, FieldIR> {
        Constructor {
            variant: self.variant.resolved(config, resolve_map, false),
            fields: self.resolved_fields(config, resolve_map),
            output: self.resolved_output(config, which, resolve_map),
            type_parameters: self
                .type_parameters
                .iter()
                .map(|t| t.resolved(config, resolve_map, false))
                .collect(),
            tl_id: self.tl_id,
            original_variant: self.original_variant,
            original_output: self.original_output,
            is_function: self.is_function,
        }
    }

    fn resolved_output(
        &self,
        config: &Option<Config>,
        which: Delimiter,
        resolve_map: &TypeResolutionMap,
    ) -> TypeIR {
        if which == Delimiter::Functions && self.is_output_a_type_parameter() {
            TypeIR::type_parameter(no_conflict_ident(self.output.name().unwrap()))
        } else {
            self.output.resolved(config, resolve_map, false)
        }
    }

    fn is_output_a_type_parameter(&self) -> bool {
        let output_name = match &self.output {
            Type::Named(v) if v.len() == 1 => v[0].as_str(),
            _ => return false,
        };
        for p in &self.type_parameters {
            if p.name.as_deref() == Some(output_name) {
                return true;
            }
        }
        false
    }

    fn resolved_fields(
        &self,
        config: &Option<Config>,
        resolve_map: &TypeResolutionMap,
    ) -> Vec<FieldIR> {
        let replace_string_with_bytes = if let Some(config) = config {
            config.replace_with_bytes.contains(&self.original_variant)
        } else {
            false
        };

        let mut ret: Vec<_> = self
            .fields
            .iter()
            .map(|f| f.resolved(config, resolve_map, replace_string_with_bytes))
            .collect();
        if &self.original_variant == "manual.gzip_packed" {
            ret.push(FieldIR::extra_default(
                "unpacked",
                ["TLObject"].iter().collect::<TypeName>().transformed(|t| quote!(Option<#t>)),
            ));
        }
        ret
    }
}

impl Constructor<TypeIR, FieldIR> {
    fn fields_tokens(&self, pub_: TokenStream, trailer: TokenStream) -> TokenStream {
        let pub_ = repeat(pub_);
        if self.fields.is_empty() {
            quote! { #trailer }
        } else {
            let fields = self.fields.iter().filter(|f| !f.ty.is_flags()).map(FieldIR::as_field);
            quote! {
                { #( #pub_ #fields , )* }
            }
        }
    }

    fn generics<F>(&self, mut param_cb: F) -> TokenStream
    where
        F: FnMut(syn::Ident) -> TokenStream,
    {
        if self.type_parameters.is_empty() {
            return quote!();
        }
        let bounds = self.type_parameters.iter().map(move |p| param_cb(p.name()));
        quote! { <#(#bounds),*> }
    }

    fn impl_generics(&self) -> TokenStream {
        self.generics(|ty| quote!(#ty))
    }

    fn rpc_generics(&self) -> TokenStream {
        self.generics(|ty| quote!(#ty: crate::Function))
    }

    fn deserialize_tlobject_generics(&self) -> TokenStream {
        self.generics(|_| quote!(crate::ton::TLObject))
    }

    fn serialize_generics(&self) -> TokenStream {
        self.generics(|ty| quote!(#ty: crate::AnyBoxedSerialize))
    }

    fn as_struct_determine_flags(&self, field_prefix: TokenStream) -> Option<TokenStream> {
        match self.fields.iter().filter(|f| f.ty.is_flags()).count() {
            0 => return None,
            1 => (),
            n => panic!("{} flags fields found on {:?}", n, self),
        }
        let determination = {
            let fields = self.fields.iter().filter_map(|f| {
                let name = f.name();
                f.flag_bit.as_ref().map(|(_name, bit)| {
                    let is_defined = f.ty.is_defined_trailer();
                    quote! {
                        #[allow(clippy::possible_missing_else)]
                        if #field_prefix #name #is_defined {
                            _flags |= 1 << #bit;
                        }
                    }
                })
            });
            quote! {
                let mut _flags = 0u32;
                #( #fields )*
            }
        };
        Some(determination)
    }

    fn as_struct_doc(&self, matched: &str) -> String {
        format!("TL-derived from `{}`\n\n```text\n{}\n```\n", self.original_variant, matched)
    }

    fn as_struct_base(
        &self,
        config: &Option<Config>,
        name: &syn::Ident,
        matched: &str,
    ) -> TokenStream {
        let doc = self.as_struct_doc(matched);
        let derives =
            gen_derives(config, quote! { Debug, Default, Clone, PartialEq }, name.to_string());
        let impl_generics = self.impl_generics();
        let fields = match self.original_variant.as_str() {
            "rldp.messagePart" | "rldp2.messagePart" => quote! { (pub crate::RldpChunk); },
            _ => self.fields_tokens(quote! {pub}, quote! {;}),
        };
        let signing_trait_impl = self.as_signing_trait_impl();
        quote! {
            #[derive(#derives)]
            #[doc = #doc]
            pub struct #name #impl_generics #fields
            impl Eq for #name {}
            #signing_trait_impl
        }
    }

    fn as_struct_deserialize(&self) -> TokenStream {
        if self.fields.is_empty() {
            return quote!(Ok(Self {}));
        }
        let mut names = Vec::<syn::Ident>::new();
        let mut reads = Vec::<TokenStream>::new();
        for f in &self.fields {
            let read_method = f.ty.as_read_method();
            let mut read_op = if f.ty.needs_determiner {
                let (tokens, bare) = f.ty.contained();
                if bare {
                    quote!(<Vec<#tokens> as crate::ton::VectoredBare<#tokens>>::deserialize(_de)?)
                } else {
                    quote!(<Vec<#tokens> as crate::ton::VectoredBoxed<#tokens>>::deserialize(_de)?)
                }
            } else {
                quote!(_de. #read_method ()?)
            };
            if f.ty.needs_box {
                read_op = quote!(Box::new(#read_op));
            }
            let expr = if let Some((flag_name, flag_bit)) = &f.flag_bit {
                let flag_ident = no_conflict_ident(flag_name);
                let predicate = quote!(#flag_ident & (1 << #flag_bit) != 0);
                if f.ty.is_unit() {
                    predicate
                } else {
                    quote! {
                        if #predicate {
                            Some(#read_op)
                        } else {
                            None
                        }
                    }
                }
            } else {
                quote!(#read_op)
            };
            let name = f.name();
            if !f.ty.is_flags() {
                names.push(f.name());
            }
            reads.push(quote!(let #name = #expr;))
        }

        quote!({
            #( #reads )*
            Ok(Self { #( #names, )* })
        })
    }

    fn as_into_boxed(&self, name: &syn::Ident) -> Option<TokenStream> {
        if self.tl_id().is_none() || self.is_function {
            return None;
        }
        let constructor = self.output.unboxed();
        let variant_name = self.full_variant_name();
        let operation = if self.variant.needs_box {
            quote!(#constructor::#variant_name(Box::new(self)))
        } else {
            quote!(#constructor::#variant_name(self))
        };
        Some(quote! {
            impl crate::IntoBoxed for #name {
                type Boxed = #constructor;
                fn into_boxed(self) -> #constructor {
                    #operation
                }
            }
        })
    }

    fn as_signing_trait_impl(&self) -> TokenStream {
        for f in &self.fields {
            if f.name() == "signature" {
                if f.ty.field_type().to_string() == "crate :: ton :: bytes" {
                    let name = self.variant_name();
                    return quote!(
                        impl crate::Signing for #name {
                            fn signature_mut(&mut self) -> &mut crate::ton::bytes { &mut self.signature }
                        }
                    );
                } else {
                    break;
                }
            }
        }
        quote!()
    }

    fn as_type_impl(
        &self,
        name: &syn::Ident,
        serialize: TokenStream,
        deserialize: Option<TokenStream>,
    ) -> TokenStream {
        let serialize_generics = self.serialize_generics();
        let impl_generics = self.impl_generics();

        let deserialize = match deserialize {
            Some(body) => {
                let deserialize_generics = self.deserialize_tlobject_generics();
                quote! {
                    impl crate::BareDeserialize for #name #deserialize_generics {
                        fn deserialize_bare(_de: &mut crate::Deserializer) -> crate::Result<Self> {
                            #body
                        }
                    }
                }
            }
            None => quote!(),
        };
        let constructor = self.tl_id().unwrap_or_else(|| quote!(unreachable!()));

        quote! {
            impl crate::Constructor for #name {
                fn constructor_const() -> u32 {
                    #constructor
                }
            }

            impl #serialize_generics crate::BareSerialize for #name #impl_generics {
                fn constructor(&self) -> u32 {
                    Self::constructor_const()
                }
                fn serialize_bare(&self, _ser: &mut crate::Serializer) -> crate::Result<()> {
                    #serialize
                }
            }

            #deserialize
        }
    }

    fn as_type_struct_base(&self, config: &Option<Config>, matched: &str) -> TokenStream {
        let name = self.variant_name();
        let serialize_destructure = match self.original_variant.as_str() {
            "rldp.messagePart" | "rldp2.messagePart" => quote! { let MessagePart(chunk) = self; },
            _ => self
                .as_variant_ref_destructure(&name)
                .map_or_else(|| quote!(), |d| quote! { let #d = self; }),
        };
        let serialize_stmts = match self.original_variant.as_str() {
            "rldp.messagePart" | "rldp2.messagePart" => quote! { chunk.serialize_bare(_ser)?; },
            _ => self.as_variant_serialize(),
        };
        let deserialize = match self.original_variant.as_str() {
            "rldp.messagePart" | "rldp2.messagePart" => {
                quote! { Ok(Self(crate::RldpChunk::deserialize_bare(_de)?)) }
            }
            _ => self.as_struct_deserialize(),
        };
        let type_impl = self.as_type_impl(
            &name,
            quote!(#serialize_destructure #serialize_stmts Ok(())),
            Some(quote!(#deserialize)),
        );
        let struct_block = self.as_struct_base(config, &name, matched);
        let into_boxed = self.as_into_boxed(&name);
        quote! {
            #struct_block
            #type_impl
            #into_boxed
        }
    }

    fn variant_name(&self) -> syn::Ident {
        self.variant.name()
    }

    fn full_variant_name(&self) -> syn::Ident {
        let name: String = self
            .original_variant
            .split('.')
            .map(|s| {
                let mut s = s.to_string();
                s[..1].make_ascii_uppercase();
                s
            })
            .collect::<Vec<String>>()
            .join("_");
        no_conflict_ident(&name)
    }

    fn as_variant_type_struct(&self, config: &Option<Config>, matched: &str) -> TokenStream {
        if self.variant_name() == "BlockIdExt" {
            let tl_id = self.tl_id().unwrap();
            if self.original_variant == "ton.blockIdExt" {
                return quote! {
                    pub(crate) type BlockIdExt = ton_block::BlockIdExt;
                };
            } else {
                return quote! {
                    pub(crate) type BlockIdExt = ton_block::BlockIdExt;
                    pub(crate) const TL_TAG: u32 = #tl_id;
                };
            }
        }
        if self.fields.is_empty() {
            quote!()
        } else {
            self.as_type_struct_base(config, matched)
        }
    }

    fn as_variant_ref_destructure(&self, name: &syn::Ident) -> Option<TokenStream> {
        if self.fields.is_empty() {
            return None;
        }
        let fields = self.fields.iter().filter(|f| !f.ty.is_flags()).map(|f| {
            let field_name = f.name();
            if f.ty.is_extra() {
                return quote! { #field_name: _ };
            }
            if let Some(local_name) = f.local_name() {
                let prefix = f.ty.ref_prefix();
                quote! { #field_name: #prefix #local_name }
            } else {
                quote! { #field_name }
            }
        });
        Some(quote! {
            #name { #( #fields ),* }
        })
    }

    fn as_variant_serialize(&self) -> TokenStream {
        let determine_flags = self.as_struct_determine_flags(quote!()).unwrap_or_default();
        let mut fields = Vec::new();
        for f in self.fields.iter() {
            if f.ty.is_unit() {
                continue;
            }
            let write_method = match f.ty.as_write_method() {
                Some(m) => m,
                None => continue,
            };
            let field_name = f.name();
            let local_name = f.local_name().unwrap_or_else(|| field_name.clone());
            let field = if f.ty.is_flags() {
                quote! { _ser.#write_method(&_flags)?; }
            } else if f.flag_bit.is_some() {
                let tokens = if f.ty.needs_determiner {
                    let (tokens, bare) = f.ty.contained();
                    if bare {
                        quote!((inner as &dyn crate::ton::VectoredBare<#tokens>).serialize(_ser)?;)
                    } else {
                        quote!((inner as &dyn crate::ton::VectoredBoxed<#tokens>).serialize(_ser)?;)
                    }
                } else {
                    quote!(_ser.#write_method(inner)?;)
                };
                quote! {
                    #[allow(clippy::possible_missing_else)]
                    if let Some(inner) = #local_name {
                        #tokens
                    }
                }
            } else if f.ty.needs_box {
                quote!(_ser.#write_method(#local_name.as_ref())?;)
            } else if f.ty.needs_determiner {
                let (tokens, bare) = f.ty.contained();
                if bare {
                    quote!((#local_name as &dyn crate::ton::VectoredBare<#tokens>).serialize(_ser)?;)
                } else {
                    quote!((#local_name as &dyn crate::ton::VectoredBoxed<#tokens>).serialize(_ser)?;)
                }
            } else {
                let prefix = f.ty.local_reference_prefix();
                quote!(_ser.#write_method(#prefix #local_name)?;)
            };
            fields.push(field);
        }
        quote! {
            #determine_flags
            #( #fields )*
        }
    }

    fn as_function_struct(&self, config: &Option<Config>, matched: &str) -> TokenStream {
        let name = self.variant_name();
        let tl_id = self.tl_id().unwrap();
        let rpc_generics = self.rpc_generics();
        let deserialize_generics = self.deserialize_tlobject_generics();
        let serialize_generics = self.serialize_generics();
        let impl_generics = self.impl_generics();
        let mut output_ty = self.output.boxed();
        if self.output.is_type_parameter() {
            output_ty = quote! {#output_ty::Reply};
        }
        let base = self.as_type_struct_base(config, matched);

        quote! {
            #base

            impl crate::BoxedDeserialize for #name #deserialize_generics {
                fn possible_constructors() -> Vec<u32> { vec![#tl_id] }
                fn deserialize_boxed(id: u32, de: &mut crate::Deserializer) -> crate::Result<Self> {
                    if id == #tl_id {
                        de.read_bare()
                    } else {
                        _invalid_id!(id)
                    }
                }
            }

            impl #serialize_generics crate::BoxedSerialize for #name #impl_generics {
                fn bare_object(&self) -> &dyn crate::BareSerialize {
                    self
                }
            }

            impl #rpc_generics crate::Function for #name #impl_generics {
                type Reply = #output_ty;
            }
        }
    }

    fn as_variant(&self) -> TokenStream {
        let name = self.full_variant_name();
        if self.fields.is_empty() {
            quote!(#name)
        } else {
            let type_name = self.variant.unboxed();
            if self.variant.needs_box {
                quote!(#name(Box<#type_name>))
            } else {
                quote!(#name(#type_name))
            }
        }
    }

    fn as_variant_serialize_arm(&self) -> TokenStream {
        let tl_id = self.tl_id().unwrap();
        if self.fields.is_empty() {
            quote!(=> {
                const X: crate::ConstructorOnly = crate::ConstructorOnly(#tl_id);
                &X
            })
        } else if self.variant.needs_box {
            quote!((x) => x.as_ref())
        } else {
            quote!((x) => x)
        }
    }

    fn as_variant_deserialize(&self, wrap_to_box: bool) -> TokenStream {
        if self.fields.is_empty() {
            quote!()
        } else {
            let read_method = self.variant.as_read_method();
            if wrap_to_box {
                quote!((Box::new(_de. #read_method ()?)))
            } else {
                quote!((_de. #read_method ()?))
            }
        }
    }

    fn tl_id(&self) -> Option<TokenStream> {
        self.tl_id.as_ref().map(|tl_id| {
            let tl_id: syn::LitInt = syn::parse_str(&format!("0x{:08x}", tl_id)).unwrap();
            quote!(#tl_id)
        })
    }

    fn as_dynamic_deserializer(&self) -> Option<TokenStream> {
        let tl_id = self.tl_id()?;
        let type_name = if self.is_function {
            Cow::Owned(format!("rpc.{}", self.original_variant))
        } else {
            Cow::Borrowed(&self.original_variant)
        };
        let mut ty = if &self.original_variant == "manual.gzip_packed" {
            quote!(crate::ton::TransparentGunzip)
        } else if self.is_function {
            self.variant.unboxed()
        } else {
            self.output.unboxed()
        };
        if !self.type_parameters.is_empty() {
            let generics = self.type_parameters.iter().map(|_| quote!(crate::ton::TLObject));
            ty = quote!(#ty<#(#generics),*>);
        }
        Some(quote! {
            crate::DynamicDeserializer::from::<#ty>(#tl_id, #type_name)
        })
    }
}

fn camelize<F>(
    config: &Option<Config>,
    resolve_map: &mut TypeResolutionMap,
    ty: &mut Type,
    additional_correction: F,
) -> NameChunks
where
    F: FnOnce(&mut NameChunks, &mut Vec<String>) -> bool,
{
    let names = ty.names_vec_mut().unwrap();
    let fixup_key = names.clone();
    let name = names.pop().unwrap();
    let mut new_name = NameChunks::from_name(&name).unwrap();
    let force_bare = additional_correction(&mut new_name, names);
    names.push(new_name.as_upper_camel_case());
    let type_ir = TypeIR::from_names_and_hint(config, names, force_bare);
    resolve_map.insert(fixup_key, type_ir.clone());
    resolve_map.insert(names.clone(), type_ir);
    new_name
}

fn gen_derives(config: &Option<Config>, mut derives: TokenStream, name: String) -> TokenStream {
    if let Some(config) = config {
        if let Some(add_derives) = config.additional_derives.get(&name) {
            for item in add_derives {
                derives.append(Punct::new(',', Spacing::Alone));
                derives.append(Ident::new(item.as_str(), Span::call_site()));
            }
        }
    }

    derives
}

impl<Ty, Fi> Constructors<Ty, Fi> {
    fn first_constructor(&self) -> &Constructor<Ty, Fi> {
        &self.0[0].0
    }
}

impl Constructors<Type, Field> {
    fn resolve(
        self,
        config: &Option<Config>,
        resolve_map: &TypeResolutionMap,
    ) -> Constructors<TypeIR, FieldIR> {
        Constructors({
            self.0
                .into_iter()
                .map(|Matched(c, m)| Matched(c.resolve(config, Delimiter::Types, resolve_map), m))
                .collect()
        })
    }

    fn fix_names(
        &mut self,
        config: &Option<Config>,
        base_ns: &[String],
        resolve_map: &mut TypeResolutionMap,
    ) {
        let output_name = camelize(config, resolve_map, &mut self.0[0].0.output, |_, _| false);

        let common_prefix = self
            .0
            .iter()
            .filter_map(|m| m.0.variant.name())
            .map(|n| NameChunks::from_name(n).unwrap())
            .fold(None, |a_opt: Option<NameChunks>, b| {
                Some(a_opt.map(|a| a.common_prefix_of(&b)).unwrap_or(b))
            })
            .unwrap_or_else(|| NameChunks(vec![]));
        let common_module = if common_prefix.0.is_empty() {
            output_name.as_snake_case()
        } else {
            common_prefix.as_snake_case()
        };
        for &mut Matched(ref mut c, _) in &mut self.0 {
            camelize(config, resolve_map, &mut c.variant, |name, names| {
                if !names.starts_with(base_ns) {
                    names.splice(..0, base_ns.iter().cloned());
                }
                names.push(common_module.clone());
                let was_bare = is_first_char_lowercase(&name.0[0]);
                name.trim_common_prefix(&common_prefix);
                if name.0.is_empty() {
                    name.clone_from(&common_prefix);
                }
                was_bare
            });
        }
    }
}

type MethodsTree<'a> =
    BTreeMap<&'a str, BTreeMap<&'a TypeIR, BTreeSet<&'a Constructor<TypeIR, FieldIR>>>>;

impl Constructors<TypeIR, FieldIR> {
    fn coalesce_methods(&self) -> MethodsTree<'_> {
        let mut map: MethodsTree = BTreeMap::new();
        for Matched(cons, _) in &self.0 {
            for field in &cons.fields {
                if field.ty.is_flags() {
                    continue;
                }
                map.entry(&field.name).or_default().entry(&field.ty).or_default().insert(cons);
            }
        }
        map
    }

    fn as_only_unwrap(&self, enum_name: &syn::Ident) -> Option<TokenStream> {
        let Matched(cons, _) = self.0.first()?;
        if self.0.len() != 1 || cons.fields.is_empty() {
            return None;
        }
        let cons_name = cons.full_variant_name();
        let ty = cons.variant.unboxed();
        let deref =
            if cons.variant.needs_box { Some(Punct::new('*', Spacing::Joint)) } else { None };
        Some(quote! {
            pub fn only(self) -> #ty {
                match self {
                    #enum_name::#cons_name(x) => #deref x
                }
            }
        })
    }

    fn determine_methods(&self, enum_name: &syn::Ident) -> TokenStream {
        let all_constructors = self.0.len();
        let mut methods = vec![];
        for (name, typemap) in self.coalesce_methods() {
            if typemap.len() != 1 {
                continue;
            }
            let name = no_conflict_ident(name);
            let (ty_ir, constructors) = typemap.into_iter().next().unwrap();
            let mut return_ir = ty_ir.clone();
            let exhaustive = constructors.len() == all_constructors;
            if !exhaustive {
                return_ir.with_option = true;
            }
            let value = wrap_option_value(
                !exhaustive && !ty_ir.with_option,
                ty_ir.as_field_reference(quote!(x.#name)),
            );
            let constructors = constructors.into_iter().map(|c| {
                let cons_name = c.full_variant_name();
                quote!(#enum_name::#cons_name(ref x) => #value)
            });
            let trailer = if exhaustive { quote!() } else { quote!(_ => None) };
            let ty = return_ir.field_reference_type();
            methods.push(quote! {
                pub fn #name(&self) -> #ty {
                    match self {
                        #( #constructors, )*
                        #trailer
                    }
                }
            });
        }

        methods.extend(self.as_only_unwrap(enum_name));

        if methods.is_empty() {
            quote!()
        } else {
            quote! {
                impl #enum_name {
                    #( #methods )*
                }
            }
        }
    }

    fn as_type_impl(
        &self,
        name: &syn::Ident,
        serialize: TokenStream,
        deserialize: TokenStream,
    ) -> TokenStream {
        let tl_ids = self.as_tl_ids();

        quote! {

            impl crate::BoxedSerialize for #name {
                fn bare_object(&self) -> &dyn crate::BareSerialize {
                    #serialize
                }
            }

            impl crate::BoxedDeserialize for #name {
                fn possible_constructors() -> Vec<u32> { vec![#tl_ids] }
                fn deserialize_boxed(_id: u32, _de: &mut crate::Deserializer) -> crate::Result<Self> {
                    #deserialize
                }
            }

        }
    }

    fn as_option_type_impl(&self) -> TokenStream {
        if self.0.len() != 2 {
            return quote!();
        }
        let tl_ids = self.constructors_and_tl_ids().collect::<Vec<_>>();
        let empty = tl_ids.iter().find(|&&(_, c)| c.fields.is_empty());
        let nonempty = tl_ids.iter().find(|&&(_, c)| !c.fields.is_empty());
        let (empty_id, nonempty_id, nonempty_cons) = match (empty, nonempty) {
            (Some((i_e, _)), Some((i_n, c_n))) => (i_e, i_n, c_n),
            _ => return quote!(),
        };
        let nonempty_variant = nonempty_cons.variant.unboxed();
        let nonempty_deserialize = nonempty_cons.as_variant_deserialize(false);

        quote! {

            impl crate::BoxedSerialize for Option<#nonempty_variant> {
                fn bare_object(&self) -> &dyn crate::BareSerialize {
                    match *self {
                        None => {
                            const X: crate::ConstructorOnly = crate::ConstructorOnly(#empty_id);
                            &X
                        },
                        Some(ref x) => x
                    }
                }
            }

            impl crate::BoxedDeserialize for Option<#nonempty_variant> {
                fn possible_constructors() -> Vec<u32> { vec![#empty_id, #nonempty_id] }
                fn deserialize_boxed(_id: u32, _de: &mut crate::Deserializer) -> crate::Result<Self> {
                    match _id {
                        #empty_id => Ok(None),
                        #nonempty_id => Ok(Some #nonempty_deserialize),
                        id => _invalid_id!(id),
                    }
                }
            }

        }
    }

    fn constructors_and_tl_ids<'this>(
        &'this self,
    ) -> Box<dyn 'this + Iterator<Item = (TokenStream, &'this Constructor<TypeIR, FieldIR>)>> {
        Box::new(self.0.iter().filter_map(|cm| cm.0.tl_id().map(|id| (id, &cm.0))))
    }

    fn as_serialize_match(&self, enum_name: &syn::Ident) -> TokenStream {
        let constructors = self.0.iter().map(|Matched(c, _)| {
            let variant_name = c.full_variant_name();
            let serialize = c.as_variant_serialize_arm();
            quote!(#enum_name::#variant_name #serialize)
        });
        quote! {
            match self {
                #( #constructors, )*
            }
        }
    }

    fn as_tl_ids(&self) -> TokenStream {
        let tl_ids = self.0.iter().filter_map(|cm| cm.0.tl_id());
        quote!(#( #tl_ids, )*)
    }

    fn as_deserialize_match(&self, enum_name: &syn::Ident) -> TokenStream {
        let constructors = self.constructors_and_tl_ids().map(|(tl_id, c)| {
            let variant_name = c.full_variant_name();
            let deserialize = c.as_variant_deserialize(c.variant.needs_box);
            quote!(#tl_id => Ok(#enum_name::#variant_name #deserialize))
        });
        quote! {
            match _id {
                #( #constructors, )*
                id => _invalid_id!(id),
            }
        }
    }

    fn as_enum_doc(&self) -> String {
        use std::fmt::Write;
        let mut ret =
            format!("TL-derived from `{}`\n\n```text\n", &self.first_constructor().original_output);
        for (e, cm) in self.0.iter().enumerate() {
            if e != 0 {
                ret.write_str("\n\n").unwrap();
            }
            ret.write_str(&cm.1).unwrap();
        }
        write!(ret, "\n```\n").unwrap();
        ret
    }

    fn enum_default_first(&self) -> Option<bool> {
        let Matched(cons, _) = self.0.first()?;
        Some(cons.fields.is_empty())
    }

    fn enum_default_impl(&self) -> Option<TokenStream> {
        let Matched(cons, _) = self.0.first()?;
        let name = self.first_constructor().output.name();
        let variant = cons.full_variant_name();

        if cons.fields.is_empty() {
            return Some(quote! {
                impl Default for #name {
                    fn default() -> Self {
                        #name::#variant
                    }
                }
            });
        }
        let ty = cons.variant.unboxed();

        let default =
            if cons.variant.needs_box { quote!(Box::default()) } else { quote!(#ty::default()) };

        Some(quote! {
            impl Default for #name {
                fn default() -> Self {
                    #name::#variant(#default)
                }
            }
        })
    }

    fn as_enum(&self, config: &Option<Config>) -> TokenStream {
        if self.0.iter().all(|cm| cm.0.tl_id().is_none()) {
            return quote!();
        }
        let name = self.first_constructor().output.name();
        let doc = self.as_enum_doc();
        if name == "BlockIdExt" {
            return quote! {
                pub(crate) type BlockIdExt = ton_block::BlockIdExt;
            };
        }
        let mut derives = quote! { Debug, Clone, PartialEq };
        let (default_impl, default_token);
        match self.enum_default_first() {
            Some(false) => {
                default_impl = self.enum_default_impl();
                default_token = None;
            }
            _ => {
                derives.append(Punct::new(',', Spacing::Alone));
                derives.append(Ident::new("Default", Span::call_site()));
                default_impl = None;
                default_token = Some(quote! { #[default] });
            }
        }
        let derives = gen_derives(config, derives, name.to_string());
        let variants = self.0.iter().map(|cm| cm.0.as_variant());
        let methods = match self.first_constructor().original_output.as_str() {
            "rldp.MessagePart" | "rldp2.MessagePart" => quote!(),
            _ => self.determine_methods(&name),
        };
        let type_impl = self.as_type_impl(
            &name,
            self.as_serialize_match(&name),
            self.as_deserialize_match(&name),
        );
        let option_type_impl = self.as_option_type_impl();

        quote! {
            #[allow(clippy::large_enum_variant)]
            #[derive(#derives)]
            #[doc = #doc]
            pub enum #name {
                #default_token
                #( #variants , )*
            }
            #methods
            impl Eq for #name {}
            #default_impl
            #type_impl
            #option_type_impl
        }
    }
}

pub fn generate_code_for(config: Option<Config>, input: &str, path: &Path) {
    let constructors = {
        let mut items = parser::parse_string(input).unwrap();
        filter_items(&config, &mut items);
        AllConstructors::from_matched_items(&config, items)
    };

    let prelude = quote! {
        #![allow(bare_trait_objects, unused_variables, unused_imports, non_snake_case)]
        #![allow(clippy::module_inception)]
        pub use crate::ton_prelude::*;
    };

    constructors.print_tokens(&config, prelude, path);
}

#[cfg(test)]
#[path = "./tests/tests.rs"]
mod tests;
