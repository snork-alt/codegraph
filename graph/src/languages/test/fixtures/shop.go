package shop

import (
	"errors"
	"fmt"

	money "github.com/example/money"
)

// ─── Types ────────────────────────────────────────────────────────────────────

// Price is an alias for money.Amount.
type Price = money.Amount

// Status represents the lifecycle state of an order.
type Status int

// Item condition options.
const (
	StatusPending   Status = iota
	StatusConfirmed Status = iota
	StatusShipped   Status = iota
	StatusDelivered Status = iota
)

// Defaults used across the package.
var (
	DefaultTimeout = 30
	maxRetries     = 3
)

// ─── Interfaces ───────────────────────────────────────────────────────────────

// Repository is the data-access contract.
type Repository interface {
	FindByID(id int) (*Item, error)
	Save(item *Item) error
	Delete(id int) error
}

// CachedRepository extends Repository with cache invalidation.
type CachedRepository interface {
	Repository
	Invalidate(id int)
}

// ─── Structs ──────────────────────────────────────────────────────────────────

// Item represents a product in the shop.
type Item struct {
	ID       int
	Name     string
	Price    float64
	Quantity int
	tags     []string
}

// BaseStore holds shared state.
type BaseStore struct {
	name string
}

// Store manages shop inventory.
type Store struct {
	*BaseStore
	repo    Repository
	cache   map[int]*Item
	timeout int
}

// ─── Generics ─────────────────────────────────────────────────────────────────

// Pair holds two values of the same type.
type Pair[T any] struct {
	First  T
	Second T
}

// ─── Constructors ─────────────────────────────────────────────────────────────

// NewStore creates and returns a configured Store.
func NewStore(repo Repository, name string) (*Store, error) {
	if repo == nil {
		return nil, errors.New("repo is required")
	}
	s := &Store{
		BaseStore: &BaseStore{name: name},
		repo:      repo,
		cache:     make(map[int]*Item),
	}
	return s, nil
}

// newItem is an unexported constructor.
func newItem(name string, price float64) *Item {
	return &Item{Name: name, Price: price}
}

// ─── Methods ──────────────────────────────────────────────────────────────────

// Add adds an item to the store.
func (s *Store) Add(item *Item) error {
	if item == nil {
		return fmt.Errorf("item cannot be nil")
	}
	err := s.repo.Save(item)
	if err != nil {
		return err
	}
	s.cache[item.ID] = item
	return nil
}

// Get retrieves an item by ID.
func (s *Store) Get(id int) (*Item, error) {
	if cached, ok := s.cache[id]; ok {
		return cached, nil
	}
	item, err := s.repo.FindByID(id)
	if err != nil {
		return nil, err
	}
	s.cache[id] = item
	return item, nil
}

// Name returns the store name (value receiver).
func (s BaseStore) Name() string {
	return s.name
}

// ─── Package functions ────────────────────────────────────────────────────────

// Discount calculates the discounted price.
func Discount(price float64, pct float64) float64 {
	return price * (1 - pct/100)
}

// formatPrice is unexported.
func formatPrice(price float64) string {
	return fmt.Sprintf("$%.2f", price)
}
