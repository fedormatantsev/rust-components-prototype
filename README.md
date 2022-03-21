# Rust Components
Small rust library that manages component dependencies for you.

## Example
```rust

struct ComponentD {
  a: Arc<ComponentA>,
  b: Arc<ComponentB>,
  c: Arc<ComponentD>
}

impl CreateComponent for ComponentD {
    fn create(
        component_resolver: ComponentResolver,
    ) -> ComponentFuture<Result<Arc<Self>, ComponentError>> {
        Box::pin(async move {
            // You can query any registered components within any registered component.
            // Library tracks cyclic dependencies and will gracefully exit initialization if
            //  dependency cycle occurs.
            let a = component_resolver.resolve::<ComponentA>().await?;
            let b = component_resolver.resolve::<ComponentB>().await?;
            let c = component_resolver.resolve::<ComponentC>().await?;
            Ok(Arc::new(Self { a, b, c }))
        })
    }
}

// You can destroy running async tasks of your component during destruction phase,
//  or use default noop destructor.
// Destruction order is dictated by components DAG,
//  i.e. `ComponentD` will be destroyed before components a, b, c.
impl DestroyComponent for ComponentD {}

impl ComponentName for ComponentD {
    fn component_name() -> &'static str {
        "component-d"
    }
}

// Components are singletons that are managed by `ComponentStore`.
// It might be a DB wrapper, api client, or cache, or anything else.
impl Component for ComponentD {}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  // Initializes all registered components and infer their relationships.
  let component_store = ComponentStore::builder()
    .register::<ComponentA>()
    .register::<ComponentB>()
    .register::<ComponentC>()
    .register::<ComponentD>()
    .build()
    .await?;
  
  // You can resolve components from component store itself,
  //  note that it's not an async operation.
  let c = component_store.resolve::<ComponentC>().unwrap();
  
  // Destroys `ComponentStore` and all registered components.
  component_store.destroy().await;
  Ok(())
}
```
