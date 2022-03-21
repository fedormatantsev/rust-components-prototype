use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

use crate::resolution::ComponentResolver;

pub type ComponentFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Error, Debug)]
pub enum ComponentError {
    #[error("Component `{0}` requires `{1}`, but `{0}` is a dependency of `{1}`")]
    DependencyCycle(String, String),

    #[error("Failed to initialize component")]
    InitializationFailed,
}

pub trait CreateComponent {
    fn create(
        component_resolver: ComponentResolver,
    ) -> ComponentFuture<Result<Arc<Self>, ComponentError>>;
}

pub trait DestroyComponent {
    fn destroy(&self) -> ComponentFuture<()> {
        Box::pin(std::future::ready(()))
    }
}

pub trait ComponentName {
    fn component_name() -> &'static str;
}

pub trait Component:
    CreateComponent + DestroyComponent + ComponentName + Send + Sync + 'static
{
}

pub type AnyComponent = Arc<dyn Any + Send + Sync + 'static>;
pub type ComponentDtor = Arc<dyn DestroyComponent + Send + Sync + 'static>;

#[derive(Copy, Clone)]
pub struct ComponentInfo {
    pub name: &'static str,
    pub type_id: TypeId,
}

impl PartialEq<Self> for ComponentInfo {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
    }
}

impl Eq for ComponentInfo {}

impl ComponentInfo {
    pub fn new<C: Component>() -> Self {
        ComponentInfo {
            name: C::component_name(),
            type_id: TypeId::of::<C>(),
        }
    }
}
