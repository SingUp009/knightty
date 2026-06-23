F5 は完了として扱ってよいです。

互換性資料上も、`f=24` / `f=32`、`s` / `v`、multipart、crop・offset・z-indexとの統合、straight alpha、Windows ConPTY制限、Unix smokeまで整理されています。

## 次に進む候補

次は **Phase 4-F6: Kitty delete selector拡張と画像ライフサイクル整備** が最適です。

raw形式追加後にUnicode placeholderやanimationへ進むより、まず削除・回収を完成させた方が、GPUメモリ管理とscrollbackの安定性を高められます。

### 対応候補

```text
a=d
d=a / d=A   # 全画像または全placement
d=i / d=I   # image ID
d=p / d=P   # placement ID
d=x / d=X   # column
d=y / d=Y   # row
d=z / d=Z   # z-index
```

小文字と大文字は、Kitty仕様に合わせて **soft delete / hard delete** を区別します。

* soft delete: placementだけ削除し、画像resourceは保持
* hard delete: placementと画像resourceを削除
* 参照がなくなったGPU textureは回収対象

## 実装上の重要点

### 削除は一度snapshotしてから適用

走査中にplacement Vecやimage mapを直接変更すると、複数selectorやresource回収で不整合が起きやすくなります。

```rust
struct DeletePlan {
    placement_ids: Vec<InternalPlacementId>,
    image_ids: Vec<InternalImageId>,
}
```

次の順序が安全です。

```text
selector解決
→ DeletePlan生成
→ placement削除
→ image参照数再計算
→ hard delete対象resource削除
→ GPU eviction通知
→ Damage::Full
```

### `d=p` の識別

placement IDはimage IDと組み合わせて解決する必要があります。

```text
(image_id, placement_id)
```

`p`だけで全画像横断検索するか、`i`併用時だけ限定するかは仕様に合わせて実装します。

### `d=x/y/z`

これらは現在の論理placement座標で判定します。

* `x`: anchor column
* `y`: logical row
* `z`: exact z-index
* scrollback offsetではなくterminal logical coordinates
* viewport resize後も論理位置で判定

## 必須テスト

### selector parser

* `d=a/A`
* `d=i/I`
* `d=p/P`
* `d=x/X`
* `d=y/Y`
* `d=z/Z`
* 必須parameter欠落
* 不正値
* overflow
* 未知selector

### soft / hard delete

* soft delete後に`a=p`で再配置可能
* hard delete後は`ENOENT`
* named placementだけ削除
* anonymous placementも対象になる条件
* 同じimageの一部placementだけ削除
* 最後のplacement削除後もsoft imageは保持
* hard deleteでGPU resource回収

### coordinate selector

* crop/offset付きplacement
* negative/zero/positive z
* scroll後のlogical row
* viewport外placement
* 複数一致
* 一致なし

### atomicity

* selector解析失敗時に何も削除しない
* quota状態が壊れない
* texture cache参照が残らない
* multipart partial uploadとの競合
* hard delete中のpartial upload中断
