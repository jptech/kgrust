// A template parameter skipped by naming a later one must be written in edition
// 1.0, and its default here is a class of the program: the instance records the
// class, and kg expand writes only the language's own types, whose names no
// declaration can hide.
// expect-refusal 30:10: cannot write this template argument list of `Store` in edition 1.0, which must write its argument `T` to reach the ones after: it is a type other than the language's own

class Small
{
public:
    uint8 value()
    {
        return 1;
    }
}

template <typename T = Small, auto Depth = 4, bool Wide = false>
class Store
{
    T _item;

public:
    uint8 get()
    {
        return _item.value() + Depth;
    }
}

class Main
{
    Store<Wide: true> _store;

public:
    uint8 run()
    {
        return _store.get();
    }
}

export Main;
