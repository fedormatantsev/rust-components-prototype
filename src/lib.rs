mod component;
mod resolution;

use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::component::{AnyComponent, ComponentDtor, ComponentInfo};
use crate::resolution::ComponentDAG;

pub use crate::component::{
    Component, ComponentError, ComponentFuture, ComponentName, CreateComponent, DestroyComponent,
};
pub use crate::resolution::ComponentResolver;

trait ComponentFactory: Send + Sync + 'static {
    fn create(
        &self,
        component_resolver: ComponentResolver,
    ) -> ComponentFuture<Result<(AnyComponent, ComponentDtor, ComponentInfo), ComponentError>>;

    fn component_info(&self) -> ComponentInfo;
}

struct ComponentFactoryImpl<C: Component> {
    _marker: PhantomData<C>,
}

impl<C: Component> Default for ComponentFactoryImpl<C> {
    fn default() -> Self {
        ComponentFactoryImpl {
            _marker: PhantomData,
        }
    }
}

impl<C: Component> ComponentFactory for ComponentFactoryImpl<C> {
    fn create(
        &self,
        component_resolver: ComponentResolver,
    ) -> ComponentFuture<Result<(AnyComponent, ComponentDtor, ComponentInfo), ComponentError>> {
        let info = self.component_info();
        Box::pin(async move {
            let component = C::create(component_resolver).await?;
            let dtor: ComponentDtor = component.clone();
            let component: AnyComponent = component;
            Ok((component, dtor, info))
        })
    }

    fn component_info(&self) -> ComponentInfo {
        ComponentInfo::new::<C>()
    }
}

struct DAGDestructor {
    destructors: Vec<(ComponentInfo, ComponentDtor)>,
}

impl DAGDestructor {
    pub fn new(mut destructors: Vec<(ComponentInfo, ComponentDtor)>, dag: ComponentDAG) -> Self {
        let depth = destructors.iter().fold(
            HashMap::<TypeId, usize>::default(),
            |mut state, (info, _)| {
                state.insert(
                    info.type_id,
                    usize::MAX - dag.get_transitive_dependencies(&info.type_id).len(),
                );

                state
            },
        );

        destructors.sort_by_cached_key(|(info, _)| {
            *depth.get(&info.type_id).cloned().get_or_insert(usize::MAX)
        });

        Self { destructors }
    }

    pub async fn destroy(self) {
        for (info, dtor) in self.destructors {
            println!("Destroying {}", info.name);
            dtor.destroy().await;
        }
    }
}

pub struct ComponentStore {
    components: HashMap<TypeId, AnyComponent>,
    destructor: DAGDestructor,
}

pub struct ComponentStoreBuilder {
    factories: Vec<Box<dyn ComponentFactory>>,
}

impl ComponentStoreBuilder {
    /// Registers a new component type in component store
    pub fn register<C: Component>(mut self) -> Self {
        self.factories
            .push(Box::new(ComponentFactoryImpl::<C>::default()));
        self
    }

    /// Creates component store.
    /// Effectively invokes `Component::create()` for each registered component.
    ///
    /// May return error if any component failed to initialize.
    pub async fn build(self) -> Result<ComponentStore, ComponentError> {
        let context = Arc::new(resolution::ResolutionContext::default());

        let (sender, mut receiver) = mpsc::channel(self.factories.len());

        for factory in self.factories {
            let sender = sender.clone();
            let resolver = ComponentResolver::new(context.clone(), factory.component_info());

            tokio::spawn(async move {
                let res = factory.create(resolver).await;
                if sender.send(res).await.is_err() {
                    panic!("Unable to send component to queue")
                }
            });
        }

        drop(sender);

        let mut destructors: Vec<(ComponentInfo, ComponentDtor)> = Default::default();

        while let Some(res) = receiver.recv().await {
            match res {
                Ok((component, dtor, component_info)) => {
                    context.add_component(&component_info, component);
                    destructors.push((component_info, dtor));
                }
                Err(err) => {
                    let (_, dag) = context.finalize();
                    DAGDestructor::new(destructors, dag).destroy().await;

                    return Err(err);
                }
            }
        }

        let (components, dag) = context.finalize();

        Ok(ComponentStore {
            components,
            destructor: DAGDestructor::new(destructors, dag),
        })
    }
}

impl ComponentStore {
    /// Creates component store builder
    pub fn builder() -> ComponentStoreBuilder {
        ComponentStoreBuilder {
            factories: Default::default(),
        }
    }

    /// Tries to resolve component in component store
    pub fn resolve<C: Component>(&self) -> Option<Arc<C>> {
        self.components
            .get(&TypeId::of::<C>())
            .map(|component| component.clone().downcast::<C>().unwrap())
    }

    /// Stops execution of held components.
    pub async fn destroy(self) {
        self.destructor.destroy().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestComponentA {}

    struct TestComponentB {
        a: Arc<TestComponentA>,
    }

    struct TestComponentC {
        a: Arc<TestComponentA>,
        b: Arc<TestComponentB>,
    }

    struct TestComponentD {
        b: Arc<TestComponentB>,
        c: Arc<TestComponentC>,
    }

    impl CreateComponent for TestComponentA {
        fn create(_: ComponentResolver) -> ComponentFuture<Result<Arc<Self>, ComponentError>> {
            Box::pin(std::future::ready(Ok(Arc::new(Self {}))))
        }
    }

    impl DestroyComponent for TestComponentA {}

    impl ComponentName for TestComponentA {
        fn component_name() -> &'static str {
            "TestComponentA"
        }
    }

    impl Component for TestComponentA {}

    impl CreateComponent for TestComponentB {
        fn create(
            component_resolver: ComponentResolver,
        ) -> ComponentFuture<Result<Arc<Self>, ComponentError>> {
            Box::pin(async move {
                let a = component_resolver.resolve::<TestComponentA>().await?;
                Ok(Arc::new(Self { a }))
            })
        }
    }

    impl DestroyComponent for TestComponentB {}

    impl ComponentName for TestComponentB {
        fn component_name() -> &'static str {
            "TestComponentB"
        }
    }

    impl Component for TestComponentB {}

    impl CreateComponent for TestComponentC {
        fn create(
            component_resolver: ComponentResolver,
        ) -> ComponentFuture<Result<Arc<Self>, ComponentError>> {
            Box::pin(async move {
                let a = component_resolver.resolve::<TestComponentA>().await?;
                let b = component_resolver.resolve::<TestComponentB>().await?;
                Ok(Arc::new(Self { a, b }))
            })
        }
    }

    impl DestroyComponent for TestComponentC {}

    impl ComponentName for TestComponentC {
        fn component_name() -> &'static str {
            "TestComponentC"
        }
    }

    impl Component for TestComponentC {}

    impl CreateComponent for TestComponentD {
        fn create(
            component_resolver: ComponentResolver,
        ) -> ComponentFuture<Result<Arc<Self>, ComponentError>> {
            Box::pin(async move {
                let b = component_resolver.resolve::<TestComponentB>().await?;
                let c = component_resolver.resolve::<TestComponentC>().await?;
                Ok(Arc::new(Self { b, c }))
            })
        }
    }

    impl DestroyComponent for TestComponentD {}

    impl ComponentName for TestComponentD {
        fn component_name() -> &'static str {
            "TestComponentD"
        }
    }

    impl Component for TestComponentD {}

    #[tokio::test]
    async fn test_basic() -> Result<(), anyhow::Error> {
        let component_store = ComponentStore::builder()
            .register::<TestComponentA>()
            .register::<TestComponentB>()
            .register::<TestComponentC>()
            .register::<TestComponentD>()
            .build()
            .await?;

        assert!(component_store.resolve::<TestComponentA>().is_some());
        assert!(component_store.resolve::<TestComponentB>().is_some());
        assert!(component_store.resolve::<TestComponentC>().is_some());
        assert!(component_store.resolve::<TestComponentD>().is_some());

        let dtor_order: Vec<_> = component_store
            .destructor
            .destructors
            .iter()
            .map(|(info, _)| info.type_id)
            .collect();

        assert_eq!(
            vec![
                TypeId::of::<TestComponentD>(),
                TypeId::of::<TestComponentC>(),
                TypeId::of::<TestComponentB>(),
                TypeId::of::<TestComponentA>()
            ],
            dtor_order
        );

        component_store.destroy().await;

        Ok(())
    }
}
