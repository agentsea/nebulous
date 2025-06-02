pub mod macros;

pub trait ToEntity<E> {
    fn to_entity(&self) -> E;
}

pub trait FromEntity<E>: Sized {
    fn from_entity(entity: &E) -> Self;
}
