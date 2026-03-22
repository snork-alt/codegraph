package com.example.shop;

import java.util.List;
import java.util.ArrayList;
import static java.util.Collections.sort;
import java.io.*;

/**
 * Fixture Java file exercising every construct the extractor must handle.
 */

// ── Annotation type ───────────────────────────────────────────────────────────

@interface Audited {
    String by() default "system";
}

// ── Top-level interface with generics ─────────────────────────────────────────

public interface Repository<T, ID extends Comparable<ID>> {
    T findById(ID id) throws NotFoundException;
    List<T> findAll();
}

// ── Abstract base class ───────────────────────────────────────────────────────

@Audited
public abstract class BaseEntity {
    private static long instanceCount = 0;
    protected final long id;

    public BaseEntity(long id) {
        this.id = id;
        BaseEntity.instanceCount++;
    }

    public abstract String describe();

    protected static long getInstanceCount() {
        return instanceCount;
    }
}

// ── Enum with interface + methods ─────────────────────────────────────────────

public enum Category implements Comparable<Category> {
    ELECTRONICS,
    CLOTHING,
    FOOD;

    public boolean isPhysical() {
        return this != FOOD;
    }
}

// ── Concrete class with generics, annotations, nested types ───────────────────

@Audited
@SuppressWarnings("unchecked")
public class Product<T extends Serializable> extends BaseEntity implements Repository<T, Long> {

    // constants
    public static final int MAX_NAME_LEN = 128;
    private static final String DEFAULT_CURRENCY = "USD";

    // instance fields
    @Deprecated
    private String name;
    private double price;
    private Category category;
    private List<T> tags;

    // ── Nested interface ──────────────────────────────────────────────────────

    public interface Priceable {
        double getPrice();
        default boolean isExpensive() {
            return getPrice() > 1000.0;
        }
    }

    // ── Nested static class ───────────────────────────────────────────────────

    public static class Builder<T extends Serializable> {
        private String name;
        private double price;

        public Builder<T> name(String name) {
            this.name = name;
            return this;
        }

        public Builder<T> price(double price) {
            this.price = price;
            return this;
        }

        public Product<T> build() {
            Product<T> p = new Product<>(0L);
            p.name = this.name;
            p.price = this.price;
            return p;
        }
    }

    // ── Constructors ──────────────────────────────────────────────────────────

    public Product(long id) {
        super(id);
        this.tags = new ArrayList<>();
    }

    public Product(long id, String name, double price, Category category) {
        super(id);
        this.name = name;
        this.price = price;
        this.category = category;
        this.tags = new ArrayList<>();
    }

    // ── Methods ───────────────────────────────────────────────────────────────

    @Override
    public String describe() {
        return name + " (" + category + ") @ " + price;
    }

    @Override
    public T findById(Long id) throws NotFoundException {
        if (id == null) {
            throw new NotFoundException("id must not be null");
        }
        return tags.get(0);
    }

    @Override
    public List<T> findAll() {
        List<T> result = new ArrayList<>();
        for (T tag : tags) {
            result.add(tag);
        }
        return result;
    }

    public void applyDiscount(double pct) throws IllegalArgumentException {
        if (pct < 0 || pct > 100) {
            throw new IllegalArgumentException("pct out of range");
        }
        double factor = (100.0 - pct) / 100.0;
        this.price = this.price * factor;
    }

    public static <S extends Serializable> Product<S> create(String name, double price) {
        return new Builder<S>().name(name).price(price).build();
    }

    // Lambda usage + captured variable
    public List<T> filterTags(java.util.function.Predicate<T> predicate) {
        List<T> filtered = new ArrayList<>();
        tags.forEach(tag -> {
            if (predicate.test(tag)) {
                filtered.add(tag);
            }
        });
        return filtered;
    }

    // Getters / setters
    public String getName()          { return this.name; }
    public double getPrice()         { return this.price; }
    public void   setName(String n)  { this.name = n; }
    public void   setPrice(double p) { this.price = p; }
    public Category getCategory()    { return this.category; }
}

// ── Exception class ───────────────────────────────────────────────────────────

public class NotFoundException extends RuntimeException {
    private final String reason;

    public NotFoundException(String reason) {
        super(reason);
        this.reason = reason;
    }

    public String getReason() { return reason; }
}
