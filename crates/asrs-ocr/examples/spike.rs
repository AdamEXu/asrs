fn main(){let t=std::time::Instant::now();let mut w=asrs_ocr::Worker::new().unwrap();println!("init {:?}",t.elapsed());
for f in std::env::args().skip(1){let b=std::fs::read(&f).unwrap();for r in [[865.,980.,200.,90.],[895.,905.,130.,80.],[1040.,760.,80.,40.]]{let t=std::time::Instant::now();println!("{:?} {:?}",w.recognize(&b,r),t.elapsed());}}}
