use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};

use crate::component::{AnyComponent, Component};
use crate::{ComponentError, ComponentInfo};

#[derive(Default, Clone)]
pub struct ComponentDAG {
    // Key depends on values
    dependencies: HashMap<TypeId, HashSet<TypeId>>,
}

impl ComponentDAG {
    /// Returns set of transitive dependencies plus self
    pub fn get_transitive_dependencies(&self, type_id: &TypeId) -> HashSet<TypeId> {
        let state: HashSet<TypeId> = [*type_id].into_iter().collect();
        self.dependencies
            .iter()
            .fold(state, |mut state, (elem_id, deps)| {
                if state.contains(elem_id) {
                    state.extend(deps.iter());
                }

                state
            })
    }

    /// Sets dependency of `source_info` on `dependency_info`.
    /// Returns error if dependency cycle appears as a result of operation.
    pub fn add_dependency(
        &mut self,
        source_info: &ComponentInfo,
        dependency_info: &ComponentInfo,
    ) -> Result<(), ComponentError> {
        if let Some(deps) = self.dependencies.get(&source_info.type_id) {
            if deps.contains(&dependency_info.type_id) {
                return Ok(());
            }
        }

        // Get all transitive dependencies of dependency_id to make sure
        // we are not getting dependency cycle.
        if self
            .get_transitive_dependencies(&dependency_info.type_id)
            .contains(&source_info.type_id)
        {
            return Err(ComponentError::DependencyCycle(
                source_info.name.to_string(),
                dependency_info.name.to_string(),
            ));
        }

        println!("{} depends on {}", source_info.name, dependency_info.name);
        self.dependencies
            .entry(source_info.type_id)
            .or_default()
            .insert(dependency_info.type_id);

        Ok(())
    }
}

#[derive(Default)]
struct Inner {
    wakers: HashMap<TypeId, Vec<core::task::Waker>>,
    components: HashMap<TypeId, AnyComponent>,
    dag: ComponentDAG,
}

impl Inner {
    fn add_dependency(
        &mut self,
        source_info: &ComponentInfo,
        dependency_info: &ComponentInfo,
    ) -> Result<(), ComponentError> {
        self.dag.add_dependency(source_info, dependency_info)
    }

    pub fn add_waker(
        &mut self,
        waker: core::task::Waker,
        source_info: &ComponentInfo,
        dependency_info: &ComponentInfo,
    ) -> Result<(), ComponentError> {
        self.add_dependency(source_info, dependency_info)?;

        if self.components.contains_key(&dependency_info.type_id) {
            waker.wake_by_ref();
        }

        self.wakers
            .entry(dependency_info.type_id)
            .or_default()
            .push(waker);

        Ok(())
    }

    pub fn add_component(&mut self, component_info: &ComponentInfo, component: AnyComponent) {
        if let Some(wakers) = self.wakers.get(&component_info.type_id) {
            for waker in wakers {
                waker.wake_by_ref();
            }
        }
        self.components.insert(component_info.type_id, component);
    }

    pub fn try_get_component(&self, component_id: &TypeId) -> Option<AnyComponent> {
        self.components.get(component_id).cloned()
    }

    pub fn components(&self) -> HashMap<TypeId, AnyComponent> {
        self.components.clone()
    }

    pub fn dag(&self) -> ComponentDAG {
        self.dag.clone()
    }
}

#[derive(Default)]
pub struct ResolutionContext {
    inner: RwLock<Inner>,
}

impl ResolutionContext {
    fn read_inner<Res>(&self, f: impl FnOnce(&Inner) -> Res) -> Res {
        if let Ok(inner) = self.inner.read() {
            f(&inner)
        } else {
            panic!("Failed to acquire rwlock")
        }
    }

    fn write_inner<Res>(&self, f: impl FnOnce(&mut Inner) -> Res) -> Res {
        if let Ok(mut inner) = self.inner.write() {
            f(&mut inner)
        } else {
            panic!("Failed to acquire rwlock")
        }
    }

    pub fn add_waker<C: Component>(
        &self,
        waker: core::task::Waker,
        source_info: &ComponentInfo,
    ) -> Result<(), ComponentError> {
        self.write_inner(|inner| inner.add_waker(waker, source_info, &ComponentInfo::new::<C>()))
    }

    pub fn add_component(&self, component_info: &ComponentInfo, component: AnyComponent) {
        self.write_inner(|inner| inner.add_component(component_info, component))
    }

    pub fn try_get_component<C: Component>(&self) -> Option<Arc<C>> {
        self.read_inner(|inner| {
            inner
                .try_get_component(&TypeId::of::<C>())
                .map(|component| component.downcast::<C>().unwrap())
        })
    }

    pub fn finalize(&self) -> (HashMap<TypeId, AnyComponent>, ComponentDAG) {
        self.read_inner(|inner| (inner.components(), inner.dag()))
    }
}

pub struct ResolveFuture<C: Component> {
    context: Arc<ResolutionContext>,
    source_info: ComponentInfo,
    _marker: PhantomData<C>,
}

impl<C: Component> Future for ResolveFuture<C> {
    type Output = Result<Arc<C>, ComponentError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Err(err) = self
            .context
            .add_waker::<C>(cx.waker().clone(), &self.source_info)
        {
            return Poll::Ready(Err(err));
        }

        match self.context.try_get_component::<C>() {
            Some(c) => Poll::Ready(Ok(c)),
            None => Poll::Pending,
        }
    }
}

pub struct ComponentResolver {
    context: Arc<ResolutionContext>,
    source_info: ComponentInfo,
}

impl ComponentResolver {
    pub(super) fn new(
        context: Arc<ResolutionContext>,
        source_info: ComponentInfo,
    ) -> ComponentResolver {
        Self {
            context,
            source_info,
        }
    }

    pub fn resolve<C: Component>(&self) -> ResolveFuture<C> {
        ResolveFuture {
            context: self.context.clone(),
            source_info: self.source_info,
            _marker: PhantomData,
        }
    }
}
