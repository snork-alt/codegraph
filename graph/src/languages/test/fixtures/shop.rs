// shop.rs — tree-sitter dependency-graph extractor fixture
// Covers every Rust construct the extractor handles.

// ---------------------------------------------------------------------------
// use declarations
// ---------------------------------------------------------------------------

use std::collections::HashMap;                        // simple path
use std::io::{Read, Write};                           // grouped
use std::fmt::{self, Display, Formatter};             // grouped with self
use std::sync::*;                                     // wildcard
use std::sync::Arc as SharedPtr;                      // alias
pub use std::cmp::Ordering;                           // pub use re-export

// ---------------------------------------------------------------------------
// extern crate
// ---------------------------------------------------------------------------

extern crate std as std_crate;

// ---------------------------------------------------------------------------
// top-level const and static
// ---------------------------------------------------------------------------

pub const MAX_STOCK: u32 = 1_000;
const DEFAULT_DISCOUNT: f64 = 0.05;

pub static SHOP_NAME: &str = "Rust Emporium";
pub static mut GLOBAL_TAX_RATE: f64 = 0.20;

// ---------------------------------------------------------------------------
// type alias
// ---------------------------------------------------------------------------

pub type ProductId = u64;
pub type Inventory = HashMap<ProductId, u32>;

// ---------------------------------------------------------------------------
// inline mod with content
// ---------------------------------------------------------------------------

mod pricing {
    #[allow(dead_code)]
    pub const BASE_MARKUP: f64 = 1.15;

    pub fn apply_markup(price: f64) -> f64 {
        price * BASE_MARKUP
    }

    pub fn round_to_cents(value: f64) -> f64 {
        (value * 100.0).round() / 100.0
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

pub trait Describable {
    fn description(&self) -> String;
}

/// Trait with a supertrait
pub trait Priceable: Describable {
    fn price(&self) -> f64;

    /// Default method body
    fn discounted_price(&self, discount: f64) -> f64 {
        self.price() * (1.0 - discount)
    }
}

/// Trait with associated types
pub trait Repository {
    type Item;
    type Error: fmt::Debug;

    fn find_by_id(&self, id: ProductId) -> Result<Self::Item, Self::Error>;
    fn save(&mut self, item: Self::Item) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Category {
    Electronics,                              // unit variant
    Clothing(String),                         // tuple variant
    Food { name: String, perishable: bool },  // struct variant
    #[deprecated]
    Legacy,                                   // attributed variant
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ShopError {
    NotFound(ProductId),
    OutOfStock { product_id: ProductId, requested: u32 },
    InvalidPrice(String),
}

impl Display for ShopError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ShopError::NotFound(id) => write!(f, "product {} not found", id),
            ShopError::OutOfStock { product_id, requested } => {
                write!(f, "product {} out of stock (requested {})", product_id, requested)
            }
            ShopError::InvalidPrice(msg) => write!(f, "invalid price: {}", msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub id: ProductId,
    pub name: String,
    pub category: Category,
    price: f64,
    stock: u32,
}

#[derive(Debug, Clone)]
pub struct Discount<T>
where
    T: Priceable,
{
    pub item: T,
    pub rate: f64,
}

#[derive(Debug, Default)]
pub struct Cart {
    items: Vec<(Product, u32)>,
    owner: String,
}

// ---------------------------------------------------------------------------
// inherent impl blocks
// ---------------------------------------------------------------------------

impl Product {
    pub const MINIMUM_PRICE: f64 = 0.01;

    /// Static / associated function (no self)
    pub fn new(id: ProductId, name: String, category: Category, price: f64) -> Result<Self, ShopError> {
        if price < Self::MINIMUM_PRICE {
            return Err(ShopError::InvalidPrice(format!("price {} is too low", price)));
        }
        // struct expression (struct literal instantiation)
        Ok(Product {
            id,
            name,
            category,
            price: pricing::apply_markup(price),
            stock: 0,
        })
    }

    pub fn restock(&mut self, quantity: u32) {
        // assignment to self field
        self.stock = self.stock + quantity;
    }

    pub fn price(&self) -> f64 {
        self.price
    }

    pub fn stock(&self) -> u32 {
        self.stock
    }

    /// Private method
    fn apply_tax(&self) -> f64 {
        // SAFETY: reading a static mut for demonstration only
        let rate = unsafe { GLOBAL_TAX_RATE };
        self.price * (1.0 + rate)
    }

    /// Async method
    pub async fn fetch_metadata(&self, _url: &str) -> Result<String, ShopError> {
        // await expression (simulated with an already-ready future)
        let result = async { format!("metadata for {}", self.name) }.await;
        Ok(result)
    }
}

impl Describable for Product {
    fn description(&self) -> String {
        format!("Product({}): {} @ ${:.2}", self.id, self.name, self.price)
    }
}

impl Priceable for Product {
    fn price(&self) -> f64 {
        self.price
    }
}

impl<T> Describable for Discount<T>
where
    T: Priceable,
{
    fn description(&self) -> String {
        format!("{} with {:.0}% off", self.item.description(), self.rate * 100.0)
    }
}

impl<T> Priceable for Discount<T>
where
    T: Priceable,
{
    fn price(&self) -> f64 {
        self.item.discounted_price(self.rate)
    }
}

impl Cart {
    pub fn new(owner: String) -> Self {
        // let without type annotation
        let items = Vec::new();
        Cart { items, owner }
    }

    pub fn add(&mut self, product: Product, qty: u32) {
        // let with type annotation
        let entry: (Product, u32) = (product, qty);
        self.items.push(entry);
    }

    pub fn total(&self) -> f64 {
        // closure capturing nothing
        let subtotal: f64 = self.items.iter().map(|(p, q)| p.price() * (*q as f64)).sum();
        let discount_fn = |amount: f64| -> f64 {
            if amount > 100.0 { amount * (1.0 - DEFAULT_DISCOUNT) } else { amount }
        };
        // let with initializer calling a function
        let discounted = discount_fn(subtotal);
        pricing::round_to_cents(discounted)
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }
}

// ---------------------------------------------------------------------------
// In-memory repository (uses associated types from trait)
// ---------------------------------------------------------------------------

pub struct InMemoryRepo {
    store: HashMap<ProductId, Product>,
}

impl InMemoryRepo {
    pub fn new() -> Self {
        InMemoryRepo { store: HashMap::new() }
    }
}

impl Repository for InMemoryRepo {
    type Item = Product;
    type Error = ShopError;

    fn find_by_id(&self, id: ProductId) -> Result<Product, ShopError> {
        self.store.get(&id).cloned().ok_or(ShopError::NotFound(id))
    }

    fn save(&mut self, item: Product) -> Result<(), ShopError> {
        self.store.insert(item.id, item);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Top-level free functions
// ---------------------------------------------------------------------------

/// Public free function
pub fn print_inventory(inventory: &Inventory) {
    // macro invocation: println!
    println!("=== {} Inventory ===", SHOP_NAME);
    for (id, qty) in inventory {
        println!("  product {:>6}: {} units", id, qty);
    }
}

/// Private free function
fn build_sample_inventory() -> Inventory {
    // macro invocation: vec! (used to seed keys)
    let ids: Vec<ProductId> = vec![1, 2, 3];
    let mut inv = HashMap::new();
    for id in ids {
        inv.insert(id, MAX_STOCK);
    }
    inv
}

/// Generic free function with where clause
pub fn cheapest<T>(items: &[T]) -> Option<&T>
where
    T: Priceable,
{
    items.iter().min_by(|a, b| {
        a.price().partial_cmp(&b.price()).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Async top-level function
pub async fn fetch_all_metadata(products: &[Product], base_url: &str) -> Vec<String> {
    let mut results = Vec::new();
    for p in products {
        // await expression
        if let Ok(meta) = p.fetch_metadata(base_url).await {
            results.push(meta);
        }
    }
    results
}

// ---------------------------------------------------------------------------
// cfg(test) module
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_product_creation() {
        let p = Product::new(1, "Widget".to_string(), Category::Electronics, 9.99);
        assert!(p.is_ok());
        let product = p.unwrap();
        // method calls: self.field access via methods
        assert!(product.price() > 9.99);
        assert_eq!(product.stock(), 0);
    }

    #[test]
    fn test_restock() {
        let mut p = Product::new(2, "Gadget".to_string(), Category::Clothing("M".into()), 19.99)
            .unwrap();
        p.restock(50);
        assert_eq!(p.stock(), 50);
    }

    #[test]
    fn test_cart_total() {
        let p1 = Product::new(1, "A".to_string(), Category::Electronics, 10.0).unwrap();
        let p2 = Product::new(2, "B".to_string(), Category::Electronics, 20.0).unwrap();
        let mut cart = Cart::new("alice".to_string());
        cart.add(p1, 2);
        cart.add(p2, 1);
        // macro invocation: format!
        let msg = format!("cart for {} has {} items", cart.owner(), cart.item_count());
        assert!(msg.contains("alice"));
        assert!(cart.total() > 0.0);
    }

    #[test]
    fn test_discount() {
        let p = Product::new(3, "C".to_string(), Category::Electronics, 50.0).unwrap();
        let d = Discount { item: p, rate: 0.10 };
        assert!(d.price() < d.item.price());
        let _ = d.description();
    }

    #[test]
    fn test_repository() {
        let mut repo = InMemoryRepo::new();
        let p = Product::new(42, "Thing".to_string(), Category::Electronics, 5.0).unwrap();
        repo.save(p).unwrap();
        let found = repo.find_by_id(42);
        assert!(found.is_ok());
        let missing = repo.find_by_id(99);
        assert!(matches!(missing, Err(ShopError::NotFound(99))));
    }

    #[test]
    fn test_inventory() {
        let inv = build_sample_inventory();
        assert_eq!(inv.len(), 3);
        print_inventory(&inv);
    }

    #[test]
    fn test_cheapest() {
        let p1 = Product::new(1, "Cheap".to_string(), Category::Electronics, 1.0).unwrap();
        let p2 = Product::new(2, "Expensive".to_string(), Category::Electronics, 100.0).unwrap();
        let items = vec![p2, p1];
        let c = cheapest(&items);
        assert!(c.is_some());
    }

    // closure capturing an outer variable
    #[test]
    fn test_closure_capture() {
        let tax = 0.08_f64;
        let apply_tax = |price: f64| price * (1.0 + tax);
        let result = apply_tax(10.0);
        assert!((result - 10.80).abs() < 1e-9);
    }

    #[test]
    fn test_shared_ptr() {
        // uses the `as` alias import: SharedPtr = Arc
        let p = Product::new(7, "Shared".to_string(), Category::Electronics, 3.0).unwrap();
        let arc_p: SharedPtr<Product> = SharedPtr::new(p);
        assert!(arc_p.price() > 0.0);
    }
}
