use std::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Width {
    W8,
    W16,
    W32,
    W64,
}

impl Width {
    pub const fn bytes(self) -> usize {
        match self {
            Width::W8 => 1,
            Width::W16 => 2,
            Width::W32 => 4,
            Width::W64 => 8,
        }
    }
}

pub trait Word: sealed::Sealed + Copy + Default + 'static {
    const WIDTH: Width;
}

impl Word for u8 {
    const WIDTH: Width = Width::W8;
}
impl Word for u16 {
    const WIDTH: Width = Width::W16;
}
impl Word for u32 {
    const WIDTH: Width = Width::W32;
}
impl Word for u64 {
    const WIDTH: Width = Width::W64;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap<K> {
    region_id: u64,
    offset: usize,
    len: usize,
    width: Width,
    kind: PhantomData<fn() -> K>,
}

impl<K> Cap<K> {
    pub(crate) fn mint(region_id: u64, offset: usize, len: usize, width: Width) -> Self {
        Cap {
            region_id,
            offset,
            len,
            width,
            kind: PhantomData,
        }
    }

    pub(crate) fn region_id(&self) -> u64 {
        self.region_id
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn width(&self) -> Width {
        self.width
    }
}

impl Cap<Write> {
    pub fn downgrade(&self) -> Cap<Read> {
        Cap::mint(self.region_id, self.offset, self.len, self.width)
    }
}
