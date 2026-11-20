use asrs_canvas::trace;
use asrs_canvas::{Canvas2d, Gradient, ImageData, ImageDataSource};
use asrs_js::string::JsString;
use asrs_parity::cve1::Value;

#[test]
fn trace_lines_are_canonical_cve1() {
    let mut c = Canvas2d::new(10, 10);
    c.save();
    c.set_fill_style("rgba(0, 0, 0, 0.5)");
    let mut g = Gradient::linear(0.0, -0.0, 1.5, f64::MIN_POSITIVE);
    g.add_color_stop(0.25, "#fff");
    c.set_stroke_style(g);
    c.arc_with_direction(1.0, 2.0, 3.0, 0.0, f64::INFINITY, true);
    c.fill_text(JsString::from_units(vec![0xdc00, 0x41]), 0.1, 0.2);
    c.set_line_dash(&[]);
    c.put_image_data(
        ImageDataSource::Inline(ImageData {
            width: 1,
            height: 1,
            data: vec![0, 127, 128, 255],
        }),
        0.0,
        0.0,
    );
    c.restore();
    let text = trace::encode(&c.calls);
    for line in text.lines() {
        let value = Value::decode(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(value.encode(), line);
    }
    assert_eq!(trace::encode(&trace::decode(&text).unwrap()), text);
}
