pub trait AsRaw<T> {
    fn as_raw(&self) -> T;
}

pub trait AsRawMut<T> {
    fn as_raw_mut(&mut self) -> T;
}
