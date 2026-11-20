use asrs_parity::{FnCase, Response, Value, fn_vectors};

fn identity(case: &FnCase<'_>) -> Response {
    match case.name() {
        "oracle:identity" => Response::Ok(case.arg(0).clone()),
        "oracle:call" => Response::Throw(Value::Error("RangeError".into(), "planted".into())),
        other => Response::Error(format!("unexpected {other}")),
    }
}

fn_vectors!(identity_vectors_match, "cve1/identity", identity);

fn_vectors!(
    #[should_panic(
        expected = "vectors cve1/identity: 1 of 7 cases differ\n\nline 4: {\"fn\":\"oracle:identity\",\"args\":[{\"o\":[[\"0\",{\"n\":\"4000000000000000\"}],[\"b\",{\"n\":\"3ff0000000000000\"}]]}]}\n  1 difference\n    at $.b: expected 0x3ff0000000000000 (1.0), actual 0x3ff0000000000001 (1.0000000000000002); 1 ulp apart"
    )]
    planted_mismatch_is_reported,
    "cve1/identity",
    |case: &FnCase<'_>| match case.arg(0).get("b") {
        Some(_) => Response::Ok(Value::object([
            ("0", Value::number(2.0)),
            ("b", Value::Number(0x3ff0_0000_0000_0001)),
        ])),
        None => identity(case),
    }
);
