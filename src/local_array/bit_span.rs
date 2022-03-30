use log::trace;

#[derive(Copy, Clone, Debug)]
pub struct BitSpan {
    pub bits: u32,
    pub len: u8
}

impl BitSpan {
    pub(crate) fn new(bits: u32, len: u8) -> Self {
        Self {
            bits,
            len
        }
    }

    // Increment the bit span by one and calcalate the new length.
    pub(crate) fn inc(mut self) -> Self{
        self.bits += 1;
        self.len = (32 - self.bits.leading_zeros()) as u8;
        trace!("inc result {:?}", self);
        self
    }
    
}

impl std::fmt::Binary for BitSpan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032b} (len {})", self.bits, self.len)
    }
}