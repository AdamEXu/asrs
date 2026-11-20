use std::io::{self, BufRead, BufReader, BufWriter, Write};

use asrs_parity::{Request, Response};

use crate::{functions, geometry, live_base, shared, units};

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--list") {
        for (name, _) in functions::FUNCTIONS
            .iter()
            .chain(shared::FUNCTIONS)
            .chain(units::FUNCTIONS)
            .chain(geometry::FUNCTIONS)
            .chain(live_base::FUNCTIONS)
        {
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

fn handle(line: &str) -> Response {
    let request = match Request::parse(line) {
        Ok(request) => request,
        Err(message) => return Response::Error(message),
    };
    match functions::lookup(&request.name).or_else(|| {
        shared::FUNCTIONS
            .iter()
            .chain(units::FUNCTIONS)
            .chain(live_base::FUNCTIONS)
            .find(|(n, _)| *n == request.name)
            .map(|(_, f)| *f)
    }) {
        Some(function) => function(&request.args),
        None => units::table_read(&request.name, &request.args)
            .unwrap_or_else(|| Response::Error(format!("unknown function {:?}", request.name))),
    }
}
