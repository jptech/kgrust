module documented
    { twice
    , thrice
    }

//| Doubles a value. For example:
//
// ```kanagawa
// import documented
//
// const auto six = twice(3);
// ```
//
// A key, not code, so untagged:
//
// ```
// 6D 5A 1B A6 54 0E 36 AE
// ```
inline uint8 twice(uint8 x)
{
    return x * 2;
}

/*|
    Triples a value. This example is wrong, `thrice` takes one argument:

    ```kanagawa
    import documented

    const auto nine = thrice(3, 4);
    ```
*/
inline uint8 thrice(uint8 x)
{
    return x * 3;
}
