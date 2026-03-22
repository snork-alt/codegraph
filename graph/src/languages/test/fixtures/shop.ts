import { EventEmitter } from 'events';
import type { Repository } from './types';
import * as utils from './utils';

export type ItemId = number;
export type Price = { amount: number; currency: string };

export const DEFAULT_TIMEOUT = 5000;
export const MAX_RETRIES = 3;

export enum ItemStatus {
    Pending = 'PENDING',
    Available = 'AVAILABLE',
    Discontinued = 'DISCONTINUED',
}

export interface Serializable<T> {
    serialize(): T;
    deserialize(data: T): void;
}

export interface Cacheable {
    getCacheKey(): string;
    invalidate(): void;
}

export interface Item extends Serializable<Record<string, unknown>> {
    id: ItemId;
    name: string;
    price: Price;
    quantity: number;
    status: ItemStatus;
    tags?: string[];
}

export type ItemFactory<T extends Item = Item> = (data: Partial<T>) => T;

export abstract class BaseEntity {
    protected id: number;
    private _createdAt: Date;

    constructor(id: number) {
        this.id = id;
        this._createdAt = new Date();
    }

    abstract validate(): boolean;

    getId(): number {
        return this.id;
    }
}

function withLogging(
    _target: unknown,
    key: string,
    descriptor: PropertyDescriptor,
): PropertyDescriptor {
    const original = descriptor.value;
    descriptor.value = function (...args: unknown[]) {
        console.log(`Calling ${key}`);
        return original.apply(this, args);
    };
    return descriptor;
}

export class ShopItem extends BaseEntity implements Item, Cacheable {
    public name: string;
    public price: Price;
    public quantity: number;
    public status: ItemStatus;
    public tags?: string[];
    private _cache: Map<string, unknown> = new Map();
    static defaultCurrency = 'USD';

    constructor(id: ItemId, name: string, price: Price) {
        super(id);
        this.name = name;
        this.price = price;
        this.quantity = 0;
        this.status = ItemStatus.Pending;
    }

    validate(): boolean {
        return this.price.amount >= 0 && this.quantity >= 0;
    }

    @withLogging
    updatePrice(newPrice: Price): void {
        this.price = newPrice;
        this._cache.clear();
    }

    async fetchDetails(): Promise<Record<string, unknown>> {
        const result = await utils.fetch(`/items/${this.id}`);
        return result;
    }

    getCacheKey(): string {
        return `item:${this.id}`;
    }

    invalidate(): void {
        this._cache.clear();
    }

    serialize(): Record<string, unknown> {
        return { id: this.id, name: this.name, price: this.price };
    }

    deserialize(data: Record<string, unknown>): void {
        this.name = data['name'] as string;
    }

    static create(name: string, price: Price): ShopItem {
        return new ShopItem(Date.now(), name, price);
    }
}

export class Store<T extends Item = Item> extends EventEmitter {
    private repo: Repository<T>;
    protected cache: Map<ItemId, T> = new Map();

    constructor(repo: Repository<T>) {
        super();
        this.repo = repo;
    }

    async add(item: T): Promise<void> {
        await this.repo.save(item);
        this.cache.set(item.id, item);
        this.emit('added', item);
    }

    async get(id: ItemId): Promise<T | null> {
        if (this.cache.has(id)) {
            return this.cache.get(id) ?? null;
        }
        return this.repo.findById(id);
    }

    protected clearCache(): void {
        this.cache.clear();
    }
}

export function discount(price: Price, pct: number): Price {
    return { ...price, amount: price.amount * (1 - pct / 100) };
}

export function createStore<T extends Item>(repo: Repository<T>): Store<T> {
    return new Store(repo);
}

function formatPrice(price: Price): string {
    return `${price.currency}${price.amount.toFixed(2)}`;
}
