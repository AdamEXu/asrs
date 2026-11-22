//! `oracle/src/app/patches.ts`: the colour-patch page both sides show.

pub const PATCH: u32 = 16;
pub const COLUMNS: u32 = 32;

const PALETTE: [u32; 11] = [
    0x2b66a2, 0xe5b31b, 0xaf2437, 0x80588e, 0xe48b32, 0xc0b487, 0x858584, 0x3b875a, 0xd993aa,
    0x5f4528, 0xff8c00,
];

pub fn patch_colours() -> Vec<[u8; 3]> {
    let mut colours: Vec<[u8; 3]> = Vec::with_capacity(512);
    let mut add = |c: [u8; 3]| {
        if !colours.contains(&c) {
            colours.push(c);
        }
    };
    for i in 0..8u8 {
        add([0, 1, 2].map(|bit| if i & (1 << bit) != 0 { 255 } else { 0 }));
    }
    for hex in PALETTE {
        add([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]);
    }
    for v in 0..=255u8 {
        add([v, v, v]);
    }
    let steps = [0, 51, 102, 153, 204, 255];
    for r in steps {
        for g in steps {
            for b in steps {
                add([r, g, b]);
            }
        }
    }
    drop(add);
    let mut i = 0u32;
    while colours.len() < 512 {
        let v = (i * 37 + 11) % 256;
        let c = [
            v as u8,
            ((v * 7 + 90) % 256) as u8,
            ((v * 13 + 200) % 256) as u8,
        ];
        if !colours.contains(&c) {
            colours.push(c);
        }
        i += 1;
    }
    colours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_hundred_twelve_distinct_patches_in_page_order() {
        let c = patch_colours();
        assert_eq!(c.len(), 512);
        assert_eq!(c[..3], [[0, 0, 0], [255, 0, 0], [0, 255, 0]]);
        assert_eq!(c[8], [0x2b, 0x66, 0xa2]);
        for (i, a) in c.iter().enumerate() {
            assert!(!c[i + 1..].contains(a));
        }
    }
}
