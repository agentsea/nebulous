#[macro_export]
macro_rules! impl_entity_conversion {
    (
        model = $model:ty,
        entity = $entity:ty,
        fields = [ $( $field:ident ),* $(,)? ],
        extra_to_entity = { $( $extra_to:tt )* },
        extra_from_entity = { $( $extra_from:tt )* }
    ) => {
        impl $crate::conversion::ToEntity<$entity> for $model {
            fn to_entity(&self) -> $entity {
                $entity {
                    $( $field: self.$field.clone(), )*
                    $( $extra_to )*
                }
            }
        }

        impl $crate::conversion::FromEntity<$entity> for $model {
            fn from_entity(entity: &$entity) -> Self {
                Self {
                    $( $field: entity.$field.clone(), )*
                    $( $extra_from )*
                }
            }
        }
    };
}
