# Design

## Paging

Shelves are read one page at a time.

```ts
const shelf = await client.fetchShelf(id);
```
