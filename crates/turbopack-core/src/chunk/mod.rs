pub mod availability_info;
pub mod available_assets;
pub(crate) mod evaluate;
pub mod optimize;

use std::{
    collections::HashSet,
    fmt::{Debug, Display},
    future::Future,
    marker::PhantomData,
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use turbo_tasks::{
    debug::ValueDebugFormat,
    graph::{
        GraphTraversal, GraphTraversalResult, ReverseTopological, SkipDuplicates, Visit,
        VisitControlFlow,
    },
    trace::TraceRawVcs,
    TryJoinIterExt, Value, ValueToString, Vc,
};
use turbo_tasks_fs::FileSystemPath;
use turbo_tasks_hash::DeterministicHash;

pub use self::evaluate::{EvaluatableAsset, EvaluatableAssets, EvaluateChunkingContext};
use self::{availability_info::AvailabilityInfo, optimize::optimize};
use crate::{
    asset::{Asset, Assets},
    environment::Environment,
    ident::AssetIdent,
    reference::{AssetReference, AssetReferences},
    resolve::{PrimaryResolveResult, ResolveResult},
};

/// A module id, which can be a number or string
#[turbo_tasks::value(shared)]
#[derive(Debug, Clone, Hash, Ord, PartialOrd, DeterministicHash)]
#[serde(untagged)]
pub enum ModuleId {
    Number(u32),
    String(String),
}

impl Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleId::Number(i) => write!(f, "{}", i),
            ModuleId::String(s) => write!(f, "{}", s),
        }
    }
}

#[turbo_tasks::value_impl]
impl ValueToString for ModuleId {
    #[turbo_tasks::function]
    fn to_string(&self) -> Vc<String> {
        Vc::cell(self.to_string())
    }
}

impl ModuleId {
    pub fn parse(id: &str) -> Result<ModuleId> {
        Ok(match id.parse::<u32>() {
            Ok(i) => ModuleId::Number(i),
            Err(_) => ModuleId::String(id.to_string()),
        })
    }
}

/// A list of module ids.
#[turbo_tasks::value(transparent, shared)]
pub struct ModuleIds(Vec<Vc<ModuleId>>);

/// A context for the chunking that influences the way chunks are created
#[turbo_tasks::value_trait]
pub trait ChunkingContext {
    fn context_path(self: Vc<Self>) -> Vc<FileSystemPath>;
    fn output_root(self: Vc<Self>) -> Vc<FileSystemPath>;

    // TODO remove this, a chunking context should not be bound to a specific
    // environment since this can change due to transitions in the module graph
    fn environment(self: Vc<Self>) -> Vc<Environment>;

    // TODO(alexkirsz) Remove this from the chunking context. This should be at the
    // discretion of chunking context implementors. However, we currently use this
    // in a couple of places in `turbopack-css`, so we need to remove that
    // dependency first.
    fn chunk_path(self: Vc<Self>, ident: Vc<AssetIdent>, extension: String) -> Vc<FileSystemPath>;

    // TODO(alexkirsz) Remove this from the chunking context.
    /// Reference Source Map Assets for chunks
    fn reference_chunk_source_maps(self: Vc<Self>, chunk: Vc<&'static dyn Asset>) -> Vc<bool>;

    fn can_be_in_same_chunk(
        self: Vc<Self>,
        asset_a: Vc<&'static dyn Asset>,
        asset_b: Vc<&'static dyn Asset>,
    ) -> Vc<bool>;

    fn asset_path(self: Vc<Self>, content_hash: String, extension: String) -> Vc<FileSystemPath>;

    fn is_hot_module_replacement_enabled(self: Vc<Self>) -> Vc<bool> {
        Vc::cell(false)
    }

    fn layer(self: Vc<Self>) -> Vc<String> {
        Vc::cell("".to_string())
    }

    fn with_layer(self: Vc<Self>, layer: String) -> Vc<&'static dyn ChunkingContext>;

    /// Generates an output chunk asset from an intermediate chunk asset.
    fn generate_chunk(self: Vc<Self>, chunk: Vc<&'static dyn Chunk>) -> Vc<&'static dyn Asset>;
}

/// An [Asset] that can be converted into a [Chunk].
#[turbo_tasks::value_trait]
pub trait ChunkableAsset: Asset {
    fn as_chunk(
        &self,
        context: Vc<&'static dyn ChunkingContext>,
        availability_info: Value<AvailabilityInfo>,
    ) -> Vc<&'static dyn Chunk>;

    fn as_root_chunk(
        self: Vc<Self>,
        context: Vc<&'static dyn ChunkingContext>,
    ) -> Vc<&'static dyn Chunk> {
        self.as_chunk(
            context,
            Value::new(AvailabilityInfo::Root {
                current_availability_root: Vc::upcast(self),
            }),
        )
    }
}

#[turbo_tasks::value]
pub struct ChunkGroup {
    chunking_context: Vc<&'static dyn ChunkingContext>,
    entry: Vc<&'static dyn Chunk>,
    evaluatable_assets: Vc<EvaluatableAssets>,
}

#[turbo_tasks::value(transparent)]
pub struct Chunks(Vec<Vc<&'static dyn Chunk>>);

#[turbo_tasks::value_impl]
impl ChunkGroup {
    /// Creates a chunk group from an asset as entrypoint
    #[turbo_tasks::function]
    pub fn from_asset(
        asset: Vc<&'static dyn ChunkableAsset>,
        chunking_context: Vc<&'static dyn ChunkingContext>,
        availability_info: Value<AvailabilityInfo>,
    ) -> Vc<Self> {
        Self::from_chunk(
            chunking_context,
            asset.as_chunk(chunking_context, availability_info),
        )
    }

    /// Creates a chunk group from a chunk as entrypoint
    #[turbo_tasks::function]
    pub fn from_chunk(
        chunking_context: Vc<&'static dyn ChunkingContext>,
        entry: Vc<&'static dyn Chunk>,
    ) -> Vc<Self> {
        Self::cell(ChunkGroup {
            chunking_context,
            entry,
            evaluatable_assets: EvaluatableAssets::empty(),
        })
    }

    /// Creates a chunk group from a chunk as entrypoint, with the given
    /// evaluated entries to be appended.
    ///
    /// `main_entry` will always be evaluated after all entries in
    /// `other_entries` are evaluated.
    #[turbo_tasks::function]
    pub fn evaluated(
        chunking_context: Vc<&'static dyn ChunkingContext>,
        main_entry: Vc<&'static dyn EvaluatableAsset>,
        other_entries: Vc<EvaluatableAssets>,
    ) -> Vc<Self> {
        Self::cell(ChunkGroup {
            chunking_context,
            entry: main_entry.as_root_chunk(chunking_context),
            // The main entry should always be *appended* to other entries, in order to ensure
            // it's only evaluated once all other entries are evaluated.
            evaluatable_assets: other_entries.with_entry(main_entry),
        })
    }

    /// Returns the entry chunk of this chunk group.
    #[turbo_tasks::function]
    pub async fn entry(self: Vc<Self>) -> Result<Vc<&'static dyn Chunk>> {
        Ok(self.await?.entry)
    }

    /// Lists all chunks that are in this chunk group.
    /// These chunks need to be loaded to fulfill that chunk group.
    /// All chunks should be loaded in parallel.
    #[turbo_tasks::function]
    pub async fn chunks(self: Vc<Self>) -> Result<Vc<Assets>> {
        let this = self.await?;
        let evaluatable_assets = this.evaluatable_assets.await?;

        let mut entry_chunks: HashSet<_> = evaluatable_assets
            .iter()
            .map({
                let chunking_context = this.chunking_context;
                move |evaluatable_asset| async move {
                    Ok(evaluatable_asset
                        .as_root_chunk(chunking_context)
                        .resolve()
                        .await?)
                }
            })
            .try_join()
            .await?
            .into_iter()
            .collect();

        entry_chunks.insert(this.entry.resolve().await?);

        let chunks: Vec<_> = GraphTraversal::<SkipDuplicates<ReverseTopological<_>, _>>::visit(
            entry_chunks.into_iter(),
            get_chunk_children,
        )
        .await
        .completed()?
        .into_inner()
        .into_iter()
        .collect();

        let chunks = Vc::cell(chunks);
        let chunks = optimize(chunks, self);
        let mut assets: Vec<Vc<&'static dyn Asset>> = chunks
            .await?
            .iter()
            .map(|chunk| this.chunking_context.generate_chunk(*chunk))
            .collect();

        if !evaluatable_assets.is_empty() {
            if let Some(evaluate_chunking_context) =
                Vc::try_resolve_sidecast::<&dyn EvaluateChunkingContext>(this.chunking_context)
                    .await?
            {
                assets.push(evaluate_chunking_context.evaluate_chunk(
                    this.entry,
                    Vc::cell(assets.clone()),
                    this.evaluatable_assets,
                ));
            }
        }

        Ok(Vc::cell(assets))
    }
}

/// Computes the list of all chunk children of a given chunk.
async fn get_chunk_children(
    parent: Vc<&'static dyn Chunk>,
) -> Result<impl Iterator<Item = Vc<&'static dyn Chunk>> + Send> {
    Ok(parent
        .references()
        .await?
        .iter()
        .copied()
        .map(reference_to_chunks)
        .try_join()
        .await?
        .into_iter()
        .flatten())
}

/// Get all parallel chunks from a parallel chunk reference.
async fn reference_to_chunks(
    r: Vc<&'static dyn AssetReference>,
) -> Result<impl Iterator<Item = Vc<&'static dyn Chunk>> + Send> {
    let mut result = Vec::new();
    if let Some(pc) = Vc::try_resolve_downcast::<&dyn ParallelChunkReference>(r).await? {
        if *pc.is_loaded_in_parallel().await? {
            result = r
                .resolve_reference()
                .await?
                .primary
                .iter()
                .map(|r| async move {
                    Ok(if let PrimaryResolveResult::Asset(a) = r {
                        Vc::try_resolve_sidecast::<&dyn Chunk>(*a).await?
                    } else {
                        None
                    })
                })
                .try_join()
                .await?;
        }
    }
    Ok(result.into_iter().flatten())
}

#[turbo_tasks::value_impl]
impl ValueToString for ChunkGroup {
    #[turbo_tasks::function]
    async fn to_string(&self) -> Result<Vc<String>> {
        Ok(Vc::cell(format!(
            "group for {}",
            self.entry.path().to_string().await?
        )))
    }
}

/// A chunk is one type of asset.
/// It usually contains multiple chunk items.
/// There is an optional trait [ParallelChunkReference] that
/// [AssetReference]s from a [Chunk] can implement.
/// If they implement that and [ParallelChunkReference::is_loaded_in_parallel]
/// returns true, all referenced assets (if they are [Chunk]s) are placed in the
/// same chunk group.
#[turbo_tasks::value_trait]
pub trait Chunk: Asset {
    fn chunking_context(self: Vc<Self>) -> Vc<&'static dyn ChunkingContext>;
    // TODO Once output assets have their own trait, this path() method will move
    // into that trait and ident() will be removed from that. Assets on the
    // output-level only have a path and no complex ident.
    /// The path of the chunk.
    fn path(self: Vc<Self>) -> Vc<FileSystemPath> {
        self.ident().path()
    }
}

/// see [Chunk] for explanation
#[turbo_tasks::value_trait]
pub trait ParallelChunkReference: AssetReference + ValueToString {
    fn is_loaded_in_parallel(self: Vc<Self>) -> Vc<bool>;
}

/// Specifies how a chunk interacts with other chunks when building a chunk
/// group
#[derive(
    Copy, Default, Clone, Hash, TraceRawVcs, Serialize, Deserialize, Eq, PartialEq, ValueDebugFormat,
)]
pub enum ChunkingType {
    /// Asset is always placed into the referencing chunk and loaded with it.
    Placed,
    /// A heuristic determines if the asset is placed into the referencing chunk
    /// or in a separate chunk that is loaded in parallel.
    #[default]
    PlacedOrParallel,
    /// Asset is always placed in a separate chunk that is loaded in parallel.
    Parallel,
    /// Asset is always placed in a separate chunk that is loaded in parallel.
    /// Referenced asset will not inherit the available modules, but form a
    /// new availability root.
    IsolatedParallel,
    /// Asset is placed in a separate chunk group that is referenced from the
    /// referencing chunk group, but not loaded.
    /// Note: Separate chunks need to be loaded by something external to current
    /// reference.
    Separate,
    /// An async loader is placed into the referencing chunk and loads the
    /// separate chunk group in which the asset is placed.
    SeparateAsync,
}

#[turbo_tasks::value(transparent)]
pub struct ChunkingTypeOption(Option<ChunkingType>);

/// An [AssetReference] implementing this trait and returning true for
/// [ChunkableAssetReference::is_chunkable] are considered as potentially
/// chunkable references. When all [Asset]s of such a reference implement
/// [ChunkableAsset] they are placed in [Chunk]s during chunking.
/// They are even potentially placed in the same [Chunk] when a chunk type
/// specific interface is implemented.
#[turbo_tasks::value_trait]
pub trait ChunkableAssetReference: AssetReference + ValueToString {
    fn chunking_type(self: Vc<Self>) -> Vc<ChunkingTypeOption> {
        Vc::cell(Some(ChunkingType::default()))
    }
}

/// A reference to a [Chunk]. Can be loaded in parallel, see [Chunk].
#[turbo_tasks::value]
pub struct ChunkReference {
    chunk: Vc<&'static dyn Chunk>,
    parallel: bool,
}

#[turbo_tasks::value_impl]
impl ChunkReference {
    #[turbo_tasks::function]
    pub fn new(chunk: Vc<&'static dyn Chunk>) -> Vc<Self> {
        Self::cell(ChunkReference {
            chunk,
            parallel: false,
        })
    }

    #[turbo_tasks::function]
    pub fn new_parallel(chunk: Vc<&'static dyn Chunk>) -> Vc<Self> {
        Self::cell(ChunkReference {
            chunk,
            parallel: true,
        })
    }
}

#[turbo_tasks::value_impl]
impl AssetReference for ChunkReference {
    #[turbo_tasks::function]
    fn resolve_reference(&self) -> Vc<ResolveResult> {
        ResolveResult::asset(Vc::upcast(self.chunk)).into()
    }
}

#[turbo_tasks::value_impl]
impl ValueToString for ChunkReference {
    #[turbo_tasks::function]
    async fn to_string(&self) -> Result<Vc<String>> {
        Ok(Vc::cell(format!(
            "chunk {}",
            self.chunk.ident().to_string().await?
        )))
    }
}

#[turbo_tasks::value_impl]
impl ParallelChunkReference for ChunkReference {
    #[turbo_tasks::function]
    fn is_loaded_in_parallel(&self) -> Vc<bool> {
        Vc::cell(self.parallel)
    }
}

/// A reference to multiple chunks from a [ChunkGroup]
#[turbo_tasks::value]
pub struct ChunkGroupReference {
    chunk_group: Vc<ChunkGroup>,
}

#[turbo_tasks::value_impl]
impl ChunkGroupReference {
    #[turbo_tasks::function]
    pub fn new(chunk_group: Vc<ChunkGroup>) -> Vc<Self> {
        Self::cell(ChunkGroupReference { chunk_group })
    }
}

#[turbo_tasks::value_impl]
impl AssetReference for ChunkGroupReference {
    #[turbo_tasks::function]
    async fn resolve_reference(&self) -> Result<Vc<ResolveResult>> {
        let set = self.chunk_group.chunks().await?.clone_value();
        Ok(ResolveResult::assets(set).into())
    }
}

#[turbo_tasks::value_impl]
impl ValueToString for ChunkGroupReference {
    #[turbo_tasks::function]
    async fn to_string(&self) -> Result<Vc<String>> {
        Ok(Vc::cell(format!(
            "chunk group {}",
            self.chunk_group.to_string().await?
        )))
    }
}

pub struct ChunkContentResult<I> {
    pub chunk_items: Vec<Vc<I>>,
    pub chunks: Vec<Vc<&'static dyn Chunk>>,
    pub async_chunk_groups: Vec<Vc<ChunkGroup>>,
    pub external_asset_references: Vec<Vc<&'static dyn AssetReference>>,
    pub availability_info: AvailabilityInfo,
}

#[async_trait::async_trait]
pub trait FromChunkableAsset: ChunkItem + Unpin + Debug {
    async fn from_asset(
        context: Vc<&'static dyn ChunkingContext>,
        asset: Vc<&'static dyn Asset>,
    ) -> Result<Option<Vc<Self>>>;
    async fn from_async_asset(
        context: Vc<&'static dyn ChunkingContext>,
        asset: Vc<&'static dyn ChunkableAsset>,
        availability_info: Value<AvailabilityInfo>,
    ) -> Result<Option<Vc<Self>>>;
}

pub async fn chunk_content_split<I>(
    context: Vc<&'static dyn ChunkingContext>,
    entry: Vc<&'static dyn Asset>,
    additional_entries: Option<Vc<Assets>>,
    availability_info: Value<AvailabilityInfo>,
) -> Result<ChunkContentResult<I>>
where
    I: FromChunkableAsset,
{
    chunk_content_internal_parallel(context, entry, additional_entries, availability_info, true)
        .await
        .map(|o| o.unwrap())
}

pub async fn chunk_content<I>(
    context: Vc<&'static dyn ChunkingContext>,
    entry: Vc<&'static dyn Asset>,
    additional_entries: Option<Vc<Assets>>,
    availability_info: Value<AvailabilityInfo>,
) -> Result<Option<ChunkContentResult<I>>>
where
    I: FromChunkableAsset,
{
    chunk_content_internal_parallel(context, entry, additional_entries, availability_info, false)
        .await
}

#[derive(Eq, PartialEq, Clone, Hash)]
enum ChunkContentGraphNode<I> {
    // Chunk items that are placed into the current chunk
    ChunkItem(I),
    // Asset that is already available and doesn't need to be included
    AvailableAsset(Vc<&'static dyn Asset>),
    // Chunks that are loaded in parallel to the current chunk
    Chunk(Vc<&'static dyn Chunk>),
    // Chunk groups that are referenced from the current chunk, but
    // not loaded in parallel
    AsyncChunkGroup(Vc<ChunkGroup>),
    ExternalAssetReference(Vc<&'static dyn AssetReference>),
}

#[derive(Clone, Copy)]
struct ChunkContentContext {
    chunking_context: Vc<&'static dyn ChunkingContext>,
    entry: Vc<&'static dyn Asset>,
    availability_info: Value<AvailabilityInfo>,
    split: bool,
}

async fn reference_to_graph_nodes<I>(
    context: ChunkContentContext,
    reference: Vc<&'static dyn AssetReference>,
) -> Result<
    Vec<(
        Option<(Vc<&'static dyn Asset>, ChunkingType)>,
        ChunkContentGraphNode<Vc<I>>,
    )>,
>
where
    I: FromChunkableAsset,
{
    let Some(chunkable_asset_reference) = Vc::try_resolve_downcast::<&dyn ChunkableAssetReference>(reference).await? else {
        return Ok(vec![(None, ChunkContentGraphNode::ExternalAssetReference(reference))]);
    };

    let Some(chunking_type) = *chunkable_asset_reference.chunking_type().await? else {
        return Ok(vec![(None, ChunkContentGraphNode::ExternalAssetReference(reference))]);
    };

    let result = reference.resolve_reference().await?;

    let assets = result.primary.iter().filter_map({
        |result| {
            if let PrimaryResolveResult::Asset(asset) = *result {
                return Some(asset);
            }
            None
        }
    });

    let mut graph_nodes = vec![];

    for asset in assets {
        if let Some(available_assets) = context.availability_info.available_assets() {
            if *available_assets.includes(asset).await? {
                graph_nodes.push((
                    Some((asset, chunking_type)),
                    ChunkContentGraphNode::AvailableAsset(asset),
                ));
                continue;
            }
        }

        let chunkable_asset = match Vc::try_resolve_sidecast::<&dyn ChunkableAsset>(asset).await? {
            Some(chunkable_asset) => chunkable_asset,
            _ => {
                return Ok(vec![(
                    None,
                    ChunkContentGraphNode::ExternalAssetReference(reference),
                )]);
            }
        };

        match chunking_type {
            ChunkingType::Placed => {
                if let Some(chunk_item) = I::from_asset(context.chunking_context, asset).await? {
                    graph_nodes.push((
                        Some((asset, chunking_type)),
                        ChunkContentGraphNode::ChunkItem(chunk_item),
                    ));
                } else {
                    return Err(anyhow!(
                        "Asset {} was requested to be placed into the  same chunk, but this \
                         wasn't possible",
                        asset.ident().to_string().await?
                    ));
                }
            }
            ChunkingType::Parallel => {
                let chunk =
                    chunkable_asset.as_chunk(context.chunking_context, context.availability_info);
                graph_nodes.push((
                    Some((asset, chunking_type)),
                    ChunkContentGraphNode::Chunk(chunk),
                ));
            }
            ChunkingType::IsolatedParallel => {
                let chunk = chunkable_asset.as_chunk(
                    context.chunking_context,
                    Value::new(AvailabilityInfo::Root {
                        current_availability_root: Vc::upcast(chunkable_asset),
                    }),
                );
                graph_nodes.push((
                    Some((asset, chunking_type)),
                    ChunkContentGraphNode::Chunk(chunk),
                ));
            }
            ChunkingType::PlacedOrParallel => {
                // heuristic for being in the same chunk
                if !context.split
                    && *context
                        .chunking_context
                        .can_be_in_same_chunk(context.entry, asset)
                        .await?
                {
                    // chunk item, chunk or other asset?
                    if let Some(chunk_item) = I::from_asset(context.chunking_context, asset).await?
                    {
                        graph_nodes.push((
                            Some((asset, chunking_type)),
                            ChunkContentGraphNode::ChunkItem(chunk_item),
                        ));
                        continue;
                    }
                }

                let chunk =
                    chunkable_asset.as_chunk(context.chunking_context, context.availability_info);
                graph_nodes.push((
                    Some((asset, chunking_type)),
                    ChunkContentGraphNode::Chunk(chunk),
                ));
            }
            ChunkingType::Separate => {
                graph_nodes.push((
                    Some((asset, chunking_type)),
                    ChunkContentGraphNode::AsyncChunkGroup(ChunkGroup::from_asset(
                        chunkable_asset,
                        context.chunking_context,
                        context.availability_info,
                    )),
                ));
            }
            ChunkingType::SeparateAsync => {
                if let Some(manifest_loader_item) = I::from_async_asset(
                    context.chunking_context,
                    chunkable_asset,
                    context.availability_info,
                )
                .await?
                {
                    graph_nodes.push((
                        Some((asset, chunking_type)),
                        ChunkContentGraphNode::ChunkItem(manifest_loader_item),
                    ));
                } else {
                    return Ok(vec![(
                        None,
                        ChunkContentGraphNode::ExternalAssetReference(reference),
                    )]);
                }
            }
        }
    }

    Ok(graph_nodes)
}

/// The maximum number of chunk items that can be in a chunk before we split it
/// into multiple chunks.
const MAX_CHUNK_ITEMS_COUNT: usize = 5000;

struct ChunkContentVisit<I> {
    context: ChunkContentContext,
    chunk_items_count: usize,
    processed_assets: HashSet<(ChunkingType, Vc<&'static dyn Asset>)>,
    _phantom: PhantomData<I>,
}

type ChunkItemToGraphNodesEdges<I> = impl Iterator<
    Item = (
        Option<(Vc<&'static dyn Asset>, ChunkingType)>,
        ChunkContentGraphNode<I>,
    ),
>;

type ChunkItemToGraphNodesFuture<I> = impl Future<Output = Result<ChunkItemToGraphNodesEdges<I>>>;

impl<I> Visit<ChunkContentGraphNode<Vc<I>>, ()> for ChunkContentVisit<I>
where
    I: FromChunkableAsset,
{
    type Edge = (
        Option<(Vc<&'static dyn Asset>, ChunkingType)>,
        ChunkContentGraphNode<Vc<I>>,
    );
    type EdgesIntoIter = ChunkItemToGraphNodesEdges<Vc<I>>;
    type EdgesFuture = ChunkItemToGraphNodesFuture<Vc<I>>;

    fn visit(
        &mut self,
        (option_key, node): (
            Option<(Vc<&'static dyn Asset>, ChunkingType)>,
            ChunkContentGraphNode<Vc<I>>,
        ),
    ) -> VisitControlFlow<ChunkContentGraphNode<Vc<I>>, ()> {
        let Some((asset, chunking_type)) = option_key else {
            return VisitControlFlow::Continue(node);
        };

        if !self.processed_assets.insert((chunking_type, asset)) {
            return VisitControlFlow::Skip(node);
        }

        if let ChunkContentGraphNode::ChunkItem(_) = &node {
            self.chunk_items_count += 1;

            // Make sure the chunk doesn't become too large.
            // This will hurt performance in many aspects.
            if !self.context.split && self.chunk_items_count >= MAX_CHUNK_ITEMS_COUNT {
                // Chunk is too large, cancel this algorithm and restart with splitting from the
                // start.
                return VisitControlFlow::Abort(());
            }
        }

        VisitControlFlow::Continue(node)
    }

    fn edges(&mut self, node: &ChunkContentGraphNode<Vc<I>>) -> Self::EdgesFuture {
        let chunk_item = if let ChunkContentGraphNode::ChunkItem(chunk_item) = node {
            Some(chunk_item.clone())
        } else {
            None
        };

        let context = self.context;

        async move {
            let Some(chunk_item) = chunk_item else {
                return Ok(vec![].into_iter().flatten());
            };

            Ok(chunk_item
                .references()
                .await?
                .into_iter()
                .map(|reference| reference_to_graph_nodes::<I>(context, *reference))
                .try_join()
                .await?
                .into_iter()
                .flatten())
        }
    }
}

async fn chunk_content_internal_parallel<I>(
    chunking_context: Vc<&'static dyn ChunkingContext>,
    entry: Vc<&'static dyn Asset>,
    additional_entries: Option<Vc<Assets>>,
    availability_info: Value<AvailabilityInfo>,
    split: bool,
) -> Result<Option<ChunkContentResult<I>>>
where
    I: FromChunkableAsset,
{
    let additional_entries = if let Some(additional_entries) = additional_entries {
        additional_entries.await?.clone_value().into_iter()
    } else {
        vec![].into_iter()
    };

    let root_edges = [entry]
        .into_iter()
        .chain(additional_entries)
        .map(|entry| async move {
            Ok((
                Some((entry, ChunkingType::Placed)),
                ChunkContentGraphNode::ChunkItem(
                    I::from_asset(chunking_context, entry).await?.unwrap(),
                ),
            ))
        })
        .try_join()
        .await?;

    let context = ChunkContentContext {
        chunking_context,
        entry,
        split,
        availability_info,
    };

    let visit = ChunkContentVisit {
        context,
        chunk_items_count: 0,
        processed_assets: Default::default(),
        _phantom: PhantomData,
    };

    let GraphTraversalResult::Completed(traversal_result) =
        GraphTraversal::<ReverseTopological<_>>::visit(root_edges, visit).await else {
            return Ok(None);
        };

    let graph_nodes: Vec<_> = traversal_result?.into_iter().collect();

    let mut chunk_items = Vec::new();
    let mut chunks = Vec::new();
    let mut async_chunk_groups = Vec::new();
    let mut external_asset_references = Vec::new();

    for graph_node in graph_nodes {
        match graph_node {
            ChunkContentGraphNode::AvailableAsset(_asset) => {}
            ChunkContentGraphNode::ChunkItem(chunk_item) => {
                chunk_items.push(chunk_item);
            }
            ChunkContentGraphNode::Chunk(chunk) => {
                chunks.push(chunk);
            }
            ChunkContentGraphNode::AsyncChunkGroup(async_chunk_group) => {
                async_chunk_groups.push(async_chunk_group);
            }
            ChunkContentGraphNode::ExternalAssetReference(reference) => {
                external_asset_references.push(reference);
            }
        }
    }

    Ok(Some(ChunkContentResult {
        chunk_items,
        chunks,
        async_chunk_groups,
        external_asset_references,
        availability_info: availability_info.into_value(),
    }))
}

#[turbo_tasks::value_trait]
pub trait ChunkItem {
    /// The [AssetIdent] of the [Asset] that this [ChunkItem] was created from.
    /// For most chunk types this must uniquely identify the asset as it's the
    /// source of the module id used at runtime.
    fn asset_ident(self: Vc<Self>) -> Vc<AssetIdent>;
    /// A [ChunkItem] can describe different `references` than its original
    /// [Asset].
    /// TODO(alexkirsz) This should have a default impl that returns empty
    /// references.
    fn references(self: Vc<Self>) -> Vc<AssetReferences>;
}

#[turbo_tasks::value(transparent)]
pub struct ChunkItems(Vec<Vc<&'static dyn ChunkItem>>);
