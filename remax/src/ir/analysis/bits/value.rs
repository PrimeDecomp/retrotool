/// Disjoint masks of bits known to be zero or one. Unknown bits may vary
/// independently; transfers deliberately lose correlations between operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bits {
    zero: u32,
    one: u32,
}

impl Bits {
    pub const UNKNOWN: Self = Self { zero: 0, one: 0 };

    pub fn constant(word: u32) -> Self { Self { zero: !word, one: word } }

    pub fn exact(self) -> Option<u32> { (self.zero | self.one == u32::MAX).then_some(self.one) }

    pub fn permits(self, word: u32) -> bool { word & self.zero == 0 && word & self.one == self.one }

    pub fn is_aligned(self, bytes: u8) -> bool {
        let mask = u32::from(bytes) - 1;
        self.zero & mask == mask
    }

    pub fn join(self, other: Self) -> Self {
        Self { zero: self.zero & other.zero, one: self.one & other.one }
    }

    pub fn not(self) -> Self { Self { zero: self.one, one: self.zero } }

    pub fn reverse(self) -> Self {
        Self { zero: self.zero.reverse_bits(), one: self.one.reverse_bits() }
    }

    pub fn and(self, other: Self) -> Self {
        Self { zero: self.zero | other.zero, one: self.one & other.one }
    }

    pub fn or(self, other: Self) -> Self {
        Self { zero: self.zero & other.zero, one: self.one | other.one }
    }

    pub fn xor(self, other: Self) -> Self {
        Self {
            zero: (self.zero & other.zero) | (self.one & other.one),
            one: (self.zero & other.one) | (self.one & other.zero),
        }
    }

    /// Common high bits of a non-wrapping unsigned interval.
    pub fn unsigned_range(minimum: u32, maximum: u32) -> Self {
        debug_assert!(minimum <= maximum);
        let prefix = !low_mask(32 - (minimum ^ maximum).leading_zeros());
        Self { zero: !minimum & prefix, one: minimum & prefix }
    }

    pub fn add(self, other: Self) -> Self {
        // Exact low-bit prefixes cannot receive carry from unknown higher bits.
        let mask = self.common_low_mask(other);
        let sum = self.one.wrapping_add(other.one);
        let mut bits = Self { zero: !sum & mask, one: sum & mask };
        let minimum = u64::from(self.one) + u64::from(other.one);
        let maximum = u64::from(!self.zero) + u64::from(!other.zero);
        // If the entire interval wraps once, both endpoints still describe it.
        // An interval straddling wrap has no useful common prefix here.
        if minimum >> 32 == maximum >> 32 {
            bits.refine(Self::unsigned_range(minimum as u32, maximum as u32));
        }
        bits
    }

    pub fn multiply(self, other: Self) -> Self {
        let mask = self.common_low_mask(other);
        let product = self.one.wrapping_mul(other.one);
        let zeros = (self.zero.trailing_ones() + other.zero.trailing_ones()).min(32);
        let mut bits = Self { zero: (!product & mask) | low_mask(zeros), one: product & mask };
        let maximum = u64::from(!self.zero) * u64::from(!other.zero);
        if maximum <= u64::from(u32::MAX) {
            bits.refine(Self::unsigned_range(self.one * other.one, maximum as u32));
        }
        bits
    }

    pub fn leading_zeros(self) -> Self {
        Self::unsigned_range(self.zero.leading_ones(), self.one.leading_zeros())
    }

    pub fn shift(self, amount: u32, right: bool, signed: bool, wrap: bool) -> Self {
        if !wrap && amount >= 32 {
            return if right && signed { self.sign_padding(31) } else { Self::constant(0) };
        }
        let amount = amount & 31;
        if amount == 0 {
            return self;
        }
        if right {
            let high = !low_mask(32 - amount);
            let padding = if signed { self.sign_padding(31) } else { Self::constant(0) };
            Self {
                zero: (self.zero >> amount) | (padding.zero & high),
                one: (self.one >> amount) | (padding.one & high),
            }
        } else {
            Self { zero: (self.zero << amount) | low_mask(amount), one: self.one << amount }
        }
    }

    pub fn extract(self, offset: u32, count: u32, signed: bool) -> Self {
        if count == 0 {
            return Self::constant(0);
        }
        let padding = if signed {
            self.sign_padding(offset.saturating_add(count - 1).min(31))
        } else {
            Self::constant(0)
        };
        if offset >= 32 {
            return padding;
        }
        let mask = low_mask(count.min(32 - offset));
        Self {
            zero: ((self.zero >> offset) & mask) | (padding.zero & !mask),
            one: ((self.one >> offset) & mask) | (padding.one & !mask),
        }
    }

    fn sign_padding(self, bit: u32) -> Self {
        if self.zero & (1 << bit) != 0 {
            Self::constant(0)
        } else if self.one & (1 << bit) != 0 {
            Self::constant(u32::MAX)
        } else {
            Self::UNKNOWN
        }
    }

    fn common_low_mask(self, other: Self) -> u32 {
        low_mask(((self.zero | self.one) & (other.zero | other.one)).trailing_ones())
    }

    fn refine(&mut self, other: Self) {
        self.zero |= other.zero;
        self.one |= other.one;
        debug_assert_eq!(self.zero & self.one, 0);
    }

    #[cfg(test)]
    pub fn from_masks(zero: u32, one: u32) -> Self {
        assert_eq!(zero & one, 0);
        Self { zero, one }
    }

    #[cfg(test)]
    pub fn contains(self, word: u32) -> bool {
        word & self.zero == 0 && word & self.one == self.one
    }
}

fn low_mask(bits: u32) -> u32 { ((1u64 << bits) - 1) as u32 }
