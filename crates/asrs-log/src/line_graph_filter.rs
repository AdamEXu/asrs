/// `shared/LineGraphFilter.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineGraphFilter {
    None,
    Differentiate,
    Integrate,
}
