"""Shop module for managing inventory and orders."""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional, Protocol

logger = logging.getLogger(__name__)

# Module-level constants
STATUS_PENDING = "pending"
STATUS_CONFIRMED = "confirmed"
MAX_ITEMS = 100

# Module-level variable
_registry: dict = {}


class ItemNotFoundError(Exception):
    """Raised when an item cannot be found."""
    pass


class Repository(Protocol):
    """Data-access protocol for item storage."""

    def find_by_id(self, item_id: int) -> Optional["Item"]:
        ...

    def save(self, item: "Item") -> None:
        ...

    def delete(self, item_id: int) -> bool:
        ...


@dataclass
class Item:
    """A product in the shop."""

    name: str
    price: float
    quantity: int = 0
    tags: list = field(default_factory=list)

    def __post_init__(self) -> None:
        if self.price < 0:
            raise ValueError("price must be non-negative")

    @property
    def is_available(self) -> bool:
        """True when the item has stock."""
        return self.quantity > 0

    @classmethod
    def create(cls, name: str, price: float) -> "Item":
        """Factory method."""
        return cls(name=name, price=price)

    @staticmethod
    def validate_price(price: float) -> bool:
        """Validate that a price is non-negative."""
        return price >= 0


class Store:
    """Manages shop inventory."""

    def __init__(self, repo: Repository, name: str = "Default") -> None:
        self.repo = repo
        self.name = name
        self._cache: dict = {}
        self._created_at = datetime.now()

    def add(self, item: Item) -> None:
        """Add an item to the store."""
        if item is None:
            raise ValueError("item cannot be None")
        self.repo.save(item)
        self._cache[id(item)] = item
        logger.info("Added item: %s", item.name)

    def get(self, item_id: int) -> Optional[Item]:
        """Retrieve an item by ID."""
        if item_id in self._cache:
            return self._cache[item_id]
        return self.repo.find_by_id(item_id)

    async def refresh(self) -> None:
        """Asynchronously clear the cache."""
        await asyncio.sleep(0)
        self._cache.clear()

    def _validate(self, item: Item) -> bool:
        """Private validation helper."""
        return item.price >= 0 and item.quantity >= 0


def discount(price: float, pct: float) -> float:
    """Apply a discount percentage to a price."""
    return price * (1 - pct / 100)


async def fetch_prices(ids: list) -> dict:
    """Asynchronously fetch prices for multiple items."""
    await asyncio.sleep(0)
    return {i: 0.0 for i in ids}


def _format_price(price: float) -> str:
    return f"${price:.2f}"
