/// Minimal big unsigned integer for exact decimal expansion of f64 values.
pub struct BigUint {
    limbs: Vec<u32>,
}

impl BigUint {
    pub fn from_u64(v: u64) -> Self {
        let mut limbs = vec![v as u32, (v >> 32) as u32];
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { limbs }
    }

    pub fn shl(&mut self, bits: u32) {
        let limb_shift = (bits / 32) as usize;
        let bit_shift = bits % 32;
        if bit_shift != 0 {
            let mut carry = 0u32;
            for limb in &mut self.limbs {
                let next = *limb >> (32 - bit_shift);
                *limb = (*limb << bit_shift) | carry;
                carry = next;
            }
            if carry != 0 {
                self.limbs.push(carry);
            }
        }
        if limb_shift != 0 {
            self.limbs.splice(0..0, std::iter::repeat_n(0, limb_shift));
        }
    }

    fn mul_u32(&mut self, m: u32) {
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let v = *limb as u64 * m as u64 + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        while carry != 0 {
            self.limbs.push(carry as u32);
            carry >>= 32;
        }
    }

    pub fn mul_pow5(&mut self, mut k: u32) {
        const POW5_13: u32 = 1_220_703_125;
        while k >= 13 {
            self.mul_u32(POW5_13);
            k -= 13;
        }
        if k != 0 {
            self.mul_u32(5u32.pow(k));
        }
    }

    fn div_rem_u32(&mut self, d: u32) -> u32 {
        let mut rem = 0u64;
        for limb in self.limbs.iter_mut().rev() {
            let cur = (rem << 32) | *limb as u64;
            *limb = (cur / d as u64) as u32;
            rem = cur % d as u64;
        }
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
        rem as u32
    }

    /// ASCII digits, most significant first, "0" for zero.
    pub fn to_decimal(&self) -> Vec<u8> {
        if self.limbs.is_empty() {
            return vec![b'0'];
        }
        let mut chunks = Vec::new();
        let mut n = Self {
            limbs: self.limbs.clone(),
        };
        while !n.limbs.is_empty() {
            chunks.push(n.div_rem_u32(1_000_000_000));
        }
        let mut out = Vec::new();
        let top = chunks.pop().unwrap_or(0);
        out.extend_from_slice(top.to_string().as_bytes());
        while let Some(chunk) = chunks.pop() {
            let s = chunk.to_string();
            out.resize(out.len() + (9 - s.len()), b'0');
            out.extend_from_slice(s.as_bytes());
        }
        out
    }
}
