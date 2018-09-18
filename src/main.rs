
extern crate clang;

use std::collections::{HashMap, HashSet, BTreeMap, VecDeque, BTreeSet};
use std::ops::Bound::Included;
use std::iter::FromIterator;
use std::cmp::Ordering;
use std::path::{PathBuf};
use std::fs;
use std::io::{BufRead};
use std::io::prelude::*;
use std::io;
use clang::*;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct CanonicalPath(PathBuf);

impl CanonicalPath {
    fn new(path: PathBuf) -> Self {
        CanonicalPath(path.canonicalize().unwrap())
    }
}

#[derive(Default)]
struct SymbolDesc<'a> {
    deps: HashSet<Entity<'a>>,
    definitions: HashSet<Entity<'a>>,
}

fn get_name(entity: &Entity) -> String {
    match entity.get_name() {
        Some(name) =>
            format!("{:?}", name),
        None =>
            format!("{:?}", entity),
    }
}

fn get_path(entity: &Entity) -> PathBuf {
    let location = entity.get_location().unwrap().get_file_location();
    location.file.unwrap().get_path()
}

fn get_location(entity: &Entity) -> String {
    let location = entity.get_location().unwrap().get_file_location();
    let path = location.file.unwrap().get_path();
    let path = path.to_str().unwrap();
    format!("{}:{}", path, location.line)
}

fn visit<'a>(
    entity: Entity<'a>,
    sym_table: &mut HashMap<Entity<'a>, SymbolDesc<'a>>,
    macros: &BTreeMap<u32, Entity<'a>>
) -> SymbolDesc<'a> 
{
    let mut desc: SymbolDesc = Default::default();

    if let Some(def) = entity.get_definition() {
        desc.definitions.insert(def);
    }

    entity.visit_children(|_, child| {
        println!("Child: {}", get_name(&child));
        if let Some(def) = child.get_definition() {
            println!("Child def: {}, {:?}", get_name(&def), get_path(&def));
            if sym_table.contains_key(&def) {
                desc.deps.insert(def);
            }
            if let Some(t) = def.get_type() {
                if let Some(t) = t.get_declaration() {
                    if sym_table.contains_key(&t) {
                        desc.deps.insert(t);
                    }
                }
            }
            if let Some(t) = def.get_typedef_underlying_type() {
                if let Some(t) = t.get_declaration() {
                    if sym_table.contains_key(&t) {
                        desc.deps.insert(t);
                    }
                }
            }
        }
        EntityVisitResult::Recurse
    });

    // Here we want to see if there is any macro expansion within this function
    // so that we can add it as dependency
    // Expansion of the macro could happen in include directive as well
    let range = entity.get_range().unwrap();
    let start_line = range.get_start().get_file_location().line;
    let end_line = range.get_end().get_file_location().line;

    let mut includes = vec![];

    for (_, &child) in macros.range((Included(start_line), Included(end_line))) {
        if child.get_location().unwrap().get_file_location().file == 
                entity.get_location().unwrap().get_file_location().file 
        {
            match child.get_kind() {
                EntityKind::MacroExpansion => {
                    desc.deps.insert(child);
                },
                EntityKind::InclusionDirective => {
                    includes.push(child);
                    desc.deps.insert(child);
                },
                _ => panic!("Should not happen"),
            }
        }
    }

    // In case that there was an include inside of the function
    // we need to see if there are any macros that happen to expand inside that file
    for include in includes {
        let include_name = include.get_name().unwrap();
        for child in macros.values() {
            // This is really inefficient, but should happen rarely
            let file_path = get_path(child);
            let file_path = file_path.to_str().unwrap();

            if file_path.contains(include_name.as_str()) {
                desc.deps.insert(child.clone());
            }
        }
    }

    desc
}

fn extract_symbols<'a>(
    targets: Vec<String>, 
    sym_table: HashMap<Entity<'a>, SymbolDesc<'a>>
) -> HashSet<Entity<'a>>
{
    // Now we can do a flood fill starting with all target symbols
    let mut visited = HashSet::new();
    let mut q = VecDeque::new();

    {
        let target_names: HashSet<String> = HashSet::from_iter(targets);

        for entity in sym_table.keys() {
            if let Some(name) = entity.get_name() {
                if target_names.contains(&name) {
                    q.push_back(entity);
                    println!("Adding {} at ({}) to start list", get_name(entity), get_location(entity));
                }
            }
        }
    }

    while let Some(entity) = q.pop_front() {
        if visited.contains(entity) {
            continue
        }

        visited.insert(entity.clone());

        match entity.get_kind() {
            EntityKind::InclusionDirective | EntityKind::MacroExpansion => continue,
            _ => (),
        }

        let desc = &sym_table[&entity];

        for dep in &desc.deps {
            if !visited.contains(dep) {
                q.push_back(dep);
            }
        }

        for def in &desc.definitions {
            if !visited.contains(def) {
                q.push_back(def);
            }
        }
    }

    let used_macros = visited.iter()
        .filter(|entity| entity.get_kind() == EntityKind::MacroExpansion)
        .map(|e| e.clone())
        .collect::<HashSet<Entity>>();

    for entity in used_macros {
        visited.insert(entity.clone());
    }

    visited
}

fn main() {
    let clang = Clang::new().unwrap();
    let index = Index::new(&clang, false, false);

    let sources = vec!["examples/simple.c", "examples/simple_impl.c"]; ////vec!["../libart/src/art.c"]; //
    let targets: Vec<String> = vec![String::from("main")];

    let mut tus = vec![];
    let mut sym_table = HashMap::new();
    let mut includes = HashSet::new();
    let mut system_includes: HashMap<String, CanonicalPath> = HashMap::new();

    for source in &sources {
        println!("Parsing {}...", source);
        tus.push(index
            .parser(source)
            .detailed_preprocessing_record(true)
            .parse()
            .unwrap());
    }

    {
        // Let's generate a list of 
        //    - Global symbols
        //    - Macro definitions
        //    - Includes
        for tu in &tus {
            for child in tu.get_entity().get_children() {
                if child.is_definition() || child.is_declaration() {
                    sym_table.insert(child, Default::default());
                } else if child.get_kind() == EntityKind::InclusionDirective {
                    includes.insert(child);
                }

                if child.is_in_system_header() {
                    if let Some(location) = child.get_location() {
                        if let Some(file) = location.get_file_location().file {
                            let full_path = file.get_path();
                            let file_name = full_path.file_name().unwrap();
                            system_includes.insert(
                                String::from(file_name.to_str().unwrap()), 
                                CanonicalPath::new(full_path.clone()));
                        }
                    }
                }
            }
        }
    }

    println!("system_includes: {:?}", system_includes);

    // Let's generate a dependency graph of symbols
    for tu in &tus {
        let mut macros = BTreeMap::new();
        for child in tu.get_entity().get_children() {
            if child.is_in_system_header() {
                continue
            }
            // Note: all macro expansions are top level entity
            match child.get_kind() {
                EntityKind::MacroExpansion | EntityKind::InclusionDirective | EntityKind::MacroDefinition => {
                    let location = child.get_location().unwrap();
                    let location = location.get_expansion_location();
                    macros.insert(location.line, child);
                },
                _ => (),
            }
        }

        for child in tu.get_entity().get_children() {
            if child.is_in_system_header() {
                continue
            }
            if child.is_definition() || child.is_declaration() {
                let desc = visit(child, &mut sym_table, &macros);

                print!("{} -> ", get_name(&child));
                for dep in &desc.deps {
                    print!("{}, ", get_name(&dep));
                }
                println!();
                sym_table.insert(child, desc);
            }
        }
    }

    {
        // Now we have to attach all the definitions to the declarations.
        // We can identify declaration by a source location.

        let mut decl_to_def_table = HashMap::new();

        for (entity, desc) in sym_table.iter() {
            if entity.is_declaration() {
                let location = entity.get_location().unwrap().get_file_location();
                let entry = decl_to_def_table.entry(location).or_insert(HashSet::<Entity>::new());

                for def in &desc.definitions {
                    entry.insert(def.clone());
                }
            }
        }

        for (entity, desc) in sym_table.iter_mut() {
            if entity.is_declaration() {
                let location = entity.get_location().unwrap().get_file_location();
                let defintions = &decl_to_def_table[&location];

                for def in defintions {
                    desc.definitions.insert(def.clone());
                }
            }
        }
    }

    let extracted_symbols = extract_symbols(targets, sym_table);

    {
        // Now we have to walk the extracted symbols and recreate the diractory structure.
        #[derive(Eq, Debug, Clone)]
        struct OrdSymbol<'a>(Entity<'a>);

        impl<'a> Ord for OrdSymbol<'a> {
            fn cmp(&self, other: &Self) -> Ordering {
                let location = self.0.get_location().unwrap().get_file_location();
                let other_location = other.0.get_location().unwrap().get_file_location();
                location.line.cmp(&other_location.line)
            }
        }

        impl<'a> PartialOrd for OrdSymbol<'a> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl<'a> PartialEq for OrdSymbol<'a> {
            fn eq(&self, other: &Self) -> bool {
                let location = self.0.get_location().unwrap().get_file_location();
                let other_location = other.0.get_location().unwrap().get_file_location();
                location.line == other_location.line
            }
        }

        let (symbols_per_file, unparsable_includes) = {
            let mut ret: HashMap<CanonicalPath, BTreeSet<OrdSymbol>> = HashMap::new();
            let mut ui = HashSet::new();

            for sym in &extracted_symbols {
                if sym.get_kind() == EntityKind::InclusionDirective {
                    ui.insert(sym.clone());
                } else {
                    let entry = ret.entry(CanonicalPath::new(get_path(sym))).or_insert(BTreeSet::<OrdSymbol>::new());
                    entry.insert(OrdSymbol(sym.clone()));
                }
            }

            (ret, ui)
        };

        let includes_per_file = {
            let mut ret = HashMap::new();
            for include in &includes {
                let path = CanonicalPath::new(get_path(include));
                let entry = ret.entry(path).or_insert(BTreeSet::<OrdSymbol>::new());
                entry.insert(OrdSymbol(include.clone()));
            }
            ret
        };

        let normalize_include_path = |entity: &Entity| -> CanonicalPath {
            let include_name = entity.get_name().unwrap();

            if let Some(full_path) = system_includes.get(&include_name) {
                full_path.clone()
            } else {
                let mut start_path = get_path(entity);
                start_path.pop();
                CanonicalPath::new(start_path.join(PathBuf::from(include_name)))
            }
        };

        let files_to_process = {
            let uifs = unparsable_includes.iter().map(normalize_include_path).collect::<HashSet<CanonicalPath>>();

            sources.iter()
                .map(|s| CanonicalPath::new(PathBuf::from(s)))
                .chain(includes.iter().map(normalize_include_path))
                .filter(|f| !uifs.contains(f))
                .filter(|path| {
                    let name = path.0.file_name().unwrap();
                    !system_includes.contains_key(&String::from(name.to_str().unwrap()))
                })
                .collect::<HashSet<CanonicalPath>>()
        };

        for (file, includes) in &includes_per_file {
            println!("includes in: {:?}", file);
            for include in includes {
                println!("  {}", get_name(&include.0));
            }
        }

        println!();
        println!("unparsable_includes: {:?}", unparsable_includes);

        for (file, symbols) in &symbols_per_file {
            println!("symbols in: {:?}", file);
            for include in symbols {
                println!("  {}", get_name(&include.0));
            }
        }

        let source_directory = PathBuf::from("examples/").canonicalize().unwrap();
        let target_directory = PathBuf::from("target_dir/");

        if !target_directory.exists() {
            fs::create_dir(&target_directory).unwrap();
        }

        for file in files_to_process {
            if !symbols_per_file.contains_key(&file) {
                continue
            }

            println!("Processing: {:?}", file);

            let mut all_output_symbols = BTreeSet::new();

            for include in &includes_per_file[&file] {
                let include_file = normalize_include_path(&include.0);

                if unparsable_includes.contains(&include.0) || !symbols_per_file.contains_key(&include_file) {
                    continue
                }

                println!("  include {}", get_name(&include.0));
                all_output_symbols.insert(include.clone());
            }

            for symbol in &symbols_per_file[&file] {
                println!("  symbol {}", get_name(&symbol.0));
                all_output_symbols.insert(symbol.clone());
            }

            let source_file = fs::File::open(&file.0).unwrap();
            let source_lines = io::BufReader::new(source_file).lines().collect::<Vec<Result<String, io::Error>>>();
            let mut target_file = fs::File::create(target_directory.join(file.0.strip_prefix(&source_directory).unwrap())).unwrap();

            for sym in all_output_symbols {
                let range = sym.0.get_range().unwrap();
                let start_line = range.get_start().get_file_location().line;
                let end_line = range.get_end().get_file_location().line;

                for line in start_line - 1 .. end_line {
                    match source_lines[line as usize] {
                        Ok(ref line) => {
                            target_file.write_all(line.as_bytes()).unwrap();
                            target_file.write("\n".as_bytes()).unwrap();
                        },
                        Err(ref why) => panic!("Couldn't read line: {}", why),
                    };
                }
            }
        }

        for include in unparsable_includes {
            let source_path = normalize_include_path(&include).0;
            let target_path = target_directory.join(source_path.strip_prefix(&source_directory).unwrap());
            fs::copy(source_path, target_path).unwrap();
        }
    }
}
