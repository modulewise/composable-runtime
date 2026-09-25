//! Build a function component by adapting an arbitrary target component.

use anyhow::Result;

pub mod deserializer;
pub mod factory;
pub mod serializer;

pub use factory::Factory;

/// Build the function component.
pub fn build(
    wit: String,
    world: Option<String>,
    function: Option<String>,
    description: Option<String>,
) -> Result<Vec<u8>> {
    composable_factory::build(&Factory::new(wit, world, function, description))
}
