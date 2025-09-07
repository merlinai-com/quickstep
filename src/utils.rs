pub trait HackLifetime {
    unsafe fn extend_lifetime<'a, 'b>(&'a self) -> &'b Self {
        std::mem::transmute(self)
    }
}

impl<T> HackLifetime for &T where T: ?Sized {}
