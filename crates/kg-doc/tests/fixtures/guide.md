# A guide with code in it

A whole file:

```kanagawa
inline uint8 twice(uint8 x)
{
    return x * 2;
}
```

Statements, checked as the body of a function:

```kanagawa
uint8 y = 3;
y++;
```

Not Kanagawa, so not checked:

```sh
kg doc --check guide.md
```

What not to write:

<!-- invalid -->
```kanagawa
uint8 y = ;
```

A method, in the class `in-class.k` beside this file holds:

<!-- in-class.k -->
```kanagawa
uint8 count() { return _count; }
```

* A block in a list:

    ```kanagawa
    bool b = true;
    ```

A mistake:

```kanagawa
inline void broken()
{
    uint8 z = undefined_name;
}
```

No tag, so not checked, whatever it holds:

```
kg doc --check guide.md
```
