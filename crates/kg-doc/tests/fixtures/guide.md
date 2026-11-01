# A guide with code in it

A whole file:

```kanagawa
inline uint8 twice(uint8 x)
{
    return x * 2;
}
```

Statements, checked as the body of a function:

```
uint8 y = 3;
y++;
```

Not Kanagawa, so not checked:

```sh
kg doc --check guide.md
```

What not to write:

<!-- invalid -->
```
uint8 y = ;
```

A method, in the class `in-class.k` beside this file holds:

<!-- in-class.k -->
```
uint8 count() { return _count; }
```

* A block in a list:

    ```
    bool b = true;
    ```

A mistake:

```
inline void broken()
{
    uint8 z = undefined_name;
}
```
