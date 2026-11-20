use std::io::{self, BufRead, BufReader, BufWriter, Write};

use asrs_parity::{Request, Response};

use crate::{assets, functions, geometry, live_base, shared, state, units};

type Table = &'static [(&'static str, functions::Function)];

const TABLES: [Table; 6] = [
    shared::FUNCTIONS,
    units::FUNCTIONS,
    geometry::FUNCTIONS,
    live_base::FUNCTIONS,
    assets::FUNCTIONS,
    state::FUNCTIONS,
];

fn table_entries() -> impl Iterator<Item = &'static (&'static str, functions::Function)> {
    TABLES.into_iter().flatten()
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--list") {
        for (name, _) in functions::FUNCTIONS.iter().chain(table_entries()) {
            println!("{name}");
        }
        return Ok(());
    }
    let mut input = BufReader::new(io::stdin());
    let mut output = BufWriter::new(io::stdout().lock());
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = input
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text.chars().all(char::is_whitespace) {
            continue;
        }
        writeln!(output, "{}", handle(text).encode()).map_err(|e| e.to_string())?;
        if input.buffer().is_empty() {
            output.flush().map_err(|e| e.to_string())?;
        }
    }
    output.flush().map_err(|e| e.to_string())
}

fn lookup(name: &str) -> Option<functions::Function> {
    functions::lookup(name).or_else(|| table_entries().find(|(n, _)| *n == name).map(|(_, f)| *f))
}

fn handle(line: &str) -> Response {
    let request = match Request::parse(line) {
        Ok(request) => request,
        Err(message) => return Response::Error(message),
    };
    match lookup(&request.name) {
        Some(function) => function(&request.args),
        None => units::table_read(&request.name, &request.args)
            .unwrap_or_else(|| Response::Error(format!("unknown function {:?}", request.name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_function_is_routed() {
        for (name, _) in functions::FUNCTIONS.iter().chain(table_entries()) {
            let response = handle(&format!(r#"{{"fn":"{name}","args":[]}}"#));
            assert!(
                !matches!(&response, Response::Error(m) if m.starts_with("unknown function")),
                "{name} is listed but not routed"
            );
        }
    }
}
