use crate::container::base::VecVecIndex;
use crate::traits::{AnnDataIterator, AnnDataOp};
use crate::{
    anndata::AnnData,
    backend::Backend,
    container::{Axis, AxisArrays, StackedArrayElem, StackedAxisArrays, StackedDataFrame},
    data::*,
};

use anyhow::{bail, ensure, Context, Result};
use indexmap::map::IndexMap;
use itertools::Itertools;
use parking_lot::Mutex;
use polars::prelude::{DataFrame, NamedFrom, Series};
use rayon::iter::{
    IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelIterator,
};
use std::{collections::HashMap, path::Path, sync::Arc};

pub struct AnnDataSet<B: Backend> {
    annotation: AnnData<B>,
    anndatas: StackedAnnData<B>,
}

impl<B: Backend> std::fmt::Display for AnnDataSet<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AnnDataSet object with n_obs x n_vars = {} x {} backed at '{}'",
            self.n_obs(),
            self.n_vars(),
            self.annotation.filename().display(),
        )?;
        if self.anndatas.len() > 0 {
            write!(
                f,
                "\ncontains {} AnnData objects with keys: '{}'",
                self.anndatas.len(),
                self.anndatas.keys().join("', '")
            )?;
        }
        if let Some(obs) = self
            .annotation
            .obs
            .lock()
            .as_ref()
            .map(|x| x.get_column_names())
        {
            if !obs.is_empty() {
                write!(f, "\n    obs: '{}'", obs.into_iter().join("', '"))?;
            }
        }
        if let Some(var) = self
            .annotation
            .var
            .lock()
            .as_ref()
            .map(|x| x.get_column_names())
        {
            if !var.is_empty() {
                write!(f, "\n    var: '{}'", var.into_iter().join("', '"))?;
            }
        }
        if let Some(keys) = self
            .annotation
            .uns
            .lock()
            .as_ref()
            .map(|x| x.keys().join("', '"))
        {
            if !keys.is_empty() {
                write!(f, "\n    uns: '{}'", keys)?;
            }
        }
        if let Some(keys) = self
            .annotation
            .obsm
            .lock()
            .as_ref()
            .map(|x| x.keys().join("', '"))
        {
            if !keys.is_empty() {
                write!(f, "\n    obsm: '{}'", keys)?;
            }
        }
        if let Some(keys) = self
            .annotation
            .obsp
            .lock()
            .as_ref()
            .map(|x| x.keys().join("', '"))
        {
            if !keys.is_empty() {
                write!(f, "\n    obsp: '{}'", keys)?;
            }
        }
        if let Some(keys) = self
            .annotation
            .varm
            .lock()
            .as_ref()
            .map(|x| x.keys().join("', '"))
        {
            if !keys.is_empty() {
                write!(f, "\n    varm: '{}'", keys)?;
            }
        }
        if let Some(keys) = self
            .annotation
            .varp
            .lock()
            .as_ref()
            .map(|x| x.keys().join("', '"))
        {
            if !keys.is_empty() {
                write!(f, "\n    varp: '{}'", keys)?;
            }
        }
        Ok(())
    }
}

impl<B: Backend> AnnDataSet<B> {
    pub fn get_x(&self) -> &StackedArrayElem<B> {
        self.anndatas.get_x()
    }

    pub fn get_anno(&self) -> &AnnData<B> {
        &self.annotation
    }

    pub fn new<'a, T, S, P>(data: T, filename: P, add_key: &str) -> Result<Self>
    where
        T: IntoIterator<Item = (S, AnnData<B>)>,
        S: ToString,
        P: AsRef<Path>,
    {
        let anndatas = StackedAnnData::new(data)?;
        let n_obs = anndatas.n_obs;
        let n_vars = anndatas.n_vars;

        let annotation = AnnData::new(filename, n_obs, n_vars)?;
        {
            // Set UNS. UNS includes children anndata locations.
            let (keys, filenames): (Vec<_>, Vec<_>) = anndatas
                .iter()
                .map(|(k, v)| (k.clone(), v.filename().display().to_string()))
                .unzip();
            let data = DataFrame::new(vec![
                Series::new("keys", keys),
                Series::new("file_path", filenames),
            ])?;
            annotation.add_uns("AnnDataSet", data)?;
        }
        {
            // Set OBS.
            let obs_names: DataFrameIndex = anndatas.values().flat_map(|x| x.obs_names()).collect();
            if obs_names.is_empty() {
                annotation
                    .set_obs_names((0..n_obs).into_iter().map(|x| x.to_string()).collect())?;
            } else {
                annotation.set_obs_names(obs_names)?;
            }
            let keys = Series::new(
                add_key,
                anndatas
                    .iter()
                    .map(|(k, v)| vec![k.clone(); v.n_obs()])
                    .flatten()
                    .collect::<Series>(),
            );
            annotation.set_obs(DataFrame::new(vec![keys])?)?;
        }
        {
            // Set VAR.
            let adata = anndatas.values().next().unwrap();
            if !adata.var_names().is_empty() {
                annotation.set_var_names(adata.var_names().into_iter().collect())?;
            }
        }
        Ok(Self {
            annotation,
            anndatas,
        })
    }

    pub fn open(file: B::File, adata_files_: Option<HashMap<String, String>>) -> Result<Self> {
        let annotation: AnnData<B> = AnnData::open(file)?;
        let file_path = annotation
            .filename()
            .read_link()
            .unwrap_or(annotation.filename())
            .to_path_buf();

        let df: DataFrame = annotation
            .fetch_uns("AnnDataSet")?
            .context("key 'AnnDataSet' is not present")?;
        let keys = df
            .column("keys")?
            .utf8()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let filenames = df
            .column("file_path")?
            .utf8()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let adata_files = adata_files_.unwrap_or(HashMap::new());
        let anndatas: Vec<(String, AnnData<B>)> = keys
            .into_par_iter()
            .zip(filenames)
            .map(|(k, v)| {
                let path = Path::new(adata_files.get(k).map_or(v, |x| &*x));
                let fl = if path.is_absolute() {
                    B::open(path)
                } else {
                    B::open(file_path.parent().unwrap_or(Path::new("./")).join(path))
                }?;
                Ok((k.to_string(), AnnData::open(fl)?))
            })
            .collect::<Result<_>>()?;

        if !adata_files.is_empty() {
            update_anndata_locations(&annotation, adata_files)?;
        }
        Ok(Self {
            annotation,
            anndatas: StackedAnnData::new(anndatas.into_iter())?,
        })
    }

    /// AnnDataSet will not move data across underlying AnnData objects. So the
    /// orders of rows in the resultant AnnDataSet object may not be consistent
    /// with the input `obs_indices`. This function will return a vector that can
    /// be used to reorder the `obs_indices` to match the final order of rows in
    /// the AnnDataSet.
    pub fn write_select<O: Backend, S: AsRef<[SelectInfoElem]>, P: AsRef<Path>>(
        &self,
        selection: S,
        dir: P,
    ) -> Result<Option<Vec<usize>>> {
        let file = dir.as_ref().join("_dataset.h5ads");
        let anndata_dir = dir.as_ref().join("anndatas");
        std::fs::create_dir_all(&anndata_dir)?;

        let (files, obs_idx_order) =
            self.anndatas
                .write_select::<O, _, _>(&selection, &anndata_dir, ".h5ad")?;

        if let Some(order) = obs_idx_order.as_ref() {
            let idx = BoundedSelectInfoElem::new(&selection.as_ref()[0], self.n_obs()).to_vec();
            let new_idx = order.iter().map(|i| idx[*i]).collect::<SelectInfoElem>();
            self.annotation
                .write_select::<O, _, _>([new_idx, selection.as_ref()[1].clone()], &file)?;
        } else {
            self.annotation.write_select::<O, _, _>(selection, &file)?;
        };

        let adata: AnnData<O> = AnnData::open(O::open_rw(&file)?)?;

        let parent_dir = if anndata_dir.is_absolute() {
            anndata_dir
        } else {
            Path::new("anndatas").to_path_buf()
        };

        let (keys, filenames): (Vec<_>, Vec<_>) = files
            .into_iter()
            .map(|(k, v)| (k, parent_dir.join(v.as_str()).to_str().unwrap().to_string()))
            .unzip();
        let file_loc = DataFrame::new(vec![
            Series::new("keys", keys),
            Series::new("file_path", filenames),
        ])?;
        adata.add_uns("AnnDataSet", file_loc)?;
        adata.close()?;
        Ok(obs_idx_order)
    }

    /// Convert AnnDataSet to AnnData object
    pub fn to_adata<O: Backend, P: AsRef<Path>>(&self, out: P, copy_x: bool) -> Result<AnnData<O>> {
        self.annotation.write::<O, _>(&out)?;
        let adata = AnnData::open(O::open_rw(&out)?)?;
        if copy_x {
            adata
                .set_x_from_iter::<_, ArrayData>(self.anndatas.get_x().chunked(500).map(|x| x.0))?;
        }
        Ok(adata)
    }

    /// Convert AnnDataSet to AnnData object
    pub fn into_adata(self, copy_x: bool) -> Result<AnnData<B>> {
        if copy_x {
            self.annotation
                .set_x_from_iter::<_, ArrayData>(self.anndatas.get_x().chunked(500).map(|x| x.0))?;
        }
        for ann in self.anndatas.elems.into_values() {
            ann.close()?;
        }
        Ok(self.annotation)
    }

    pub fn close(self) -> Result<()> {
        self.annotation.close()?;
        for ann in self.anndatas.elems.into_values() {
            ann.close()?;
        }
        Ok(())
    }
}

fn update_anndata_locations<B: Backend>(
    ann: &AnnData<B>,
    new_locations: HashMap<String, String>,
) -> Result<()> {
    let df: DataFrame = ann
        .fetch_uns("AnnDataSet")?
        .context("key 'AnnDataSet' is not present")?;
    let keys = df.column("keys").unwrap();
    let filenames = df
        .column("file_path")?
        .utf8()?
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .unwrap();
    let new_files: Vec<_> = keys
        .utf8()?
        .into_iter()
        .zip(filenames)
        .map(|(k, v)| {
            new_locations
                .get(k.unwrap())
                .map_or(v.to_string(), |x| x.clone())
        })
        .collect();
    let data = DataFrame::new(vec![keys.clone(), Series::new("file_path", new_files)]).unwrap();
    ann.add_uns("AnnDataSet", data)?;
    Ok(())
}

impl<B: Backend> AnnDataOp for AnnDataSet<B> {
    fn read_x<D>(&self) -> Result<Option<D>>
    where
        D: ReadData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        Ok(Some(self.anndatas.x.data()?))
    }

    fn read_x_slice<D, S>(&self, select: S) -> Result<Option<D>>
    where
        D: ReadArrayData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        S: AsRef<[SelectInfoElem]>,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        Ok(Some(self.anndatas.x.select(select.as_ref())?))
    }

    fn set_x<D: WriteData + Into<ArrayData> + HasShape>(&self, _: D) -> Result<()> {
        bail!("cannot set X in AnnDataSet")
    }

    fn del_x(&self) -> Result<()> {
        bail!("cannot delete X in AnnDataSet")
    }

    fn n_obs(&self) -> usize {
        self.anndatas.n_obs
    }
    fn n_vars(&self) -> usize {
        self.anndatas.n_vars
    }

    fn obs_ix(&self, names: &[String]) -> Result<Vec<usize>> {
        self.annotation.obs_ix(names)
    }
    fn var_ix(&self, names: &[String]) -> Result<Vec<usize>> {
        self.annotation.var_ix(names)
    }
    fn obs_names(&self) -> Vec<String> {
        self.annotation.obs_names()
    }
    fn var_names(&self) -> Vec<String> {
        self.annotation.var_names()
    }
    fn set_obs_names(&self, index: DataFrameIndex) -> Result<()> {
        self.annotation.set_obs_names(index)
    }
    fn set_var_names(&self, index: DataFrameIndex) -> Result<()> {
        self.annotation.set_var_names(index)
    }

    fn read_obs(&self) -> Result<DataFrame> {
        self.annotation.read_obs()
    }
    fn read_var(&self) -> Result<DataFrame> {
        self.annotation.read_var()
    }
    fn set_obs(&self, obs: DataFrame) -> Result<()> {
        self.annotation.set_obs(obs)
    }
    fn set_var(&self, var: DataFrame) -> Result<()> {
        self.annotation.set_var(var)
    }
    fn del_obs(&self) -> Result<()> {
        self.annotation.del_obs()
    }
    fn del_var(&self) -> Result<()> {
        self.annotation.del_var()
    }

    fn set_uns<I: Iterator<Item = (String, Data)>>(&self, data: I) -> Result<()> {
        self.annotation.set_uns(data)
    }
    fn set_obsm<I: Iterator<Item = (String, ArrayData)>>(&self, data: I) -> Result<()> {
        self.annotation.set_obsm(data)
    }
    fn set_obsp<I: Iterator<Item = (String, ArrayData)>>(&self, data: I) -> Result<()> {
        self.annotation.set_obsp(data)
    }
    fn set_varm<I: Iterator<Item = (String, ArrayData)>>(&self, data: I) -> Result<()> {
        self.annotation.set_varm(data)
    }
    fn set_varp<I: Iterator<Item = (String, ArrayData)>>(&self, data: I) -> Result<()> {
        self.annotation.set_varp(data)
    }

    fn uns_keys(&self) -> Vec<String> {
        self.annotation.uns_keys()
    }
    fn obsm_keys(&self) -> Vec<String> {
        self.annotation.obsm_keys()
    }
    fn obsp_keys(&self) -> Vec<String> {
        self.annotation.obsp_keys()
    }
    fn varm_keys(&self) -> Vec<String> {
        self.annotation.varm_keys()
    }
    fn varp_keys(&self) -> Vec<String> {
        self.annotation.varp_keys()
    }

    fn fetch_uns<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: ReadData + Into<Data> + TryFrom<Data> + Clone,
        <D as TryFrom<Data>>::Error: Into<anyhow::Error>,
    {
        self.annotation.fetch_uns(key)
    }

    fn fetch_obsm<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: ReadData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        self.annotation.fetch_obsm(key)
    }

    fn fetch_obsp<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: ReadData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        self.annotation.fetch_obsp(key)
    }

    fn fetch_varm<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: ReadData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        self.annotation.fetch_varm(key)
    }

    fn fetch_varp<D>(&self, key: &str) -> Result<Option<D>>
    where
        D: ReadData + Into<ArrayData> + TryFrom<ArrayData> + Clone,
        <D as TryFrom<ArrayData>>::Error: Into<anyhow::Error>,
    {
        self.annotation.fetch_varp(key)
    }

    fn add_uns<D: WriteData + Into<Data>>(&self, key: &str, data: D) -> Result<()> {
        self.annotation.add_uns(key, data)
    }

    fn add_obsm<D: WriteArrayData + HasShape + Into<ArrayData>>(
        &self,
        key: &str,
        data: D,
    ) -> Result<()> {
        self.annotation.add_obsm(key, data)
    }

    fn add_obsp<D: WriteArrayData + HasShape + Into<ArrayData>>(
        &self,
        key: &str,
        data: D,
    ) -> Result<()> {
        self.annotation.add_obsp(key, data)
    }

    fn add_varm<D: WriteArrayData + HasShape + Into<ArrayData>>(
        &self,
        key: &str,
        data: D,
    ) -> Result<()> {
        self.annotation.add_varm(key, data)
    }

    fn add_varp<D: WriteArrayData + HasShape + Into<ArrayData>>(
        &self,
        key: &str,
        data: D,
    ) -> Result<()> {
        self.annotation.add_varp(key, data)
    }

    fn del_uns(&self) -> Result<()> {
        self.annotation.del_uns()
    }
    fn del_obsm(&self) -> Result<()> {
        self.annotation.del_obsm()
    }
    fn del_obsp(&self) -> Result<()> {
        self.annotation.del_obsp()
    }
    fn del_varm(&self) -> Result<()> {
        self.annotation.del_varm()
    }
    fn del_varp(&self) -> Result<()> {
        self.annotation.del_varp()
    }
}

pub struct StackedAnnData<B: Backend> {
    index: Arc<Mutex<VecVecIndex>>,
    elems: IndexMap<String, AnnData<B>>,
    n_obs: usize,
    n_vars: usize,
    x: StackedArrayElem<B>,
    obs: StackedDataFrame<B>,
    obsm: StackedAxisArrays<B>,
}

impl<B: Backend> std::fmt::Display for StackedAnnData<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stacked AnnData objects:")?;
        write!(
            f,
            "\n    obs: '{}'",
            self.obs.get_column_names().iter().join("', '")
        )?;
        write!(f, "\n    obsm: '{}'", self.obsm.keys().join("', '"))?;
        Ok(())
    }
}

impl<B: Backend> StackedAnnData<B> {
    fn new<'a, T, S>(iter: T) -> Result<Self>
    where
        T: IntoIterator<Item = (S, AnnData<B>)>,
        S: ToString,
    {
        let adatas: IndexMap<String, AnnData<B>> =
            iter.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        ensure!(!adatas.is_empty(), "no AnnData objects to stack");

        if let Some((_, first)) = adatas.first() {
            let lock = first.var.lock();
            let var_names: Option<&Vec<String>> = lock.as_ref().map(|x| &x.index.names);
            if !adatas
                .par_values()
                .skip(1)
                .all(|x| x.var.lock().as_ref().map(|x| &x.index.names).eq(&var_names))
            {
                bail!("var names mismatch");
            }
        }

        let x = StackedArrayElem::new(adatas.values().map(|x| x.get_x().clone()).collect())?;

        let obs = if adatas.values().any(|x| x.obs.is_empty()) {
            StackedDataFrame::new(Vec::new())
        } else {
            StackedDataFrame::new(adatas.values().map(|x| x.obs.clone()).collect())
        }?;

        let obsm = {
            let arrays: Vec<AxisArrays<_>> = adatas.values().map(|x| x.obsm.clone()).collect();
            StackedAxisArrays::new(Axis::Row, arrays)?
        };

        Ok(Self {
            index: Arc::new(Mutex::new(x.get_index().clone())),
            n_obs: adatas.values().map(|x| x.n_obs()).sum(),
            n_vars: adatas.values().next().unwrap().n_vars(),
            elems: adatas,
            x,
            obs,
            obsm,
        })
    }

    pub fn get_x(&self) -> &StackedArrayElem<B> {
        &self.x
    }
    pub fn get_obsm(&self) -> &StackedAxisArrays<B> {
        &self.obsm
    }

    pub fn len(&self) -> usize {
        self.elems.len()
    }

    pub fn keys(&self) -> indexmap::map::Keys<'_, String, AnnData<B>> {
        self.elems.keys()
    }

    pub fn values(&self) -> indexmap::map::Values<'_, String, AnnData<B>> {
        self.elems.values()
    }

    pub fn iter(&self) -> indexmap::map::Iter<'_, String, AnnData<B>> {
        self.elems.iter()
    }

    /// Write a part of stacked AnnData objects to disk, return the key and
    /// file name (without parent paths)
    pub fn write_select<O, S, P>(
        &self,
        selection: S,
        dir: P,
        suffix: &str,
    ) -> Result<(IndexMap<String, String>, Option<Vec<usize>>)>
    where
        O: Backend,
        S: AsRef<[SelectInfoElem]>,
        P: AsRef<Path> + std::marker::Sync,
    {
        let index = self.index.lock();
        let slice = selection.as_ref();
        ensure!(slice.len() == 2, "selection must be 2D");

        let (slices, mapping) = index.split_select(&slice[0]);

        let files: Result<_> = self
            .elems
            .par_iter()
            .enumerate()
            .flat_map(|(i, (k, adata))| {
                if let Some(s) = slices.get(&i) {
                    let file = dir.as_ref().join(k.to_string() + suffix);
                    let filename = (
                        k.clone(),
                        file.file_name().unwrap().to_str().unwrap().to_string(),
                    );
                    Some(
                        adata
                            .write_select::<O, _, _>([s.clone(), slice[1].clone()], file)
                            .map(|_| filename),
                    )
                } else {
                    None
                }
            })
            .collect();

        let order = mapping.map(|x| {
            let n = self.n_obs;
            let len = slices.values().map(|x| BoundedSelectInfoElem::new(x, n).len()).sum();
            reverse_mapping(x.as_slice(), len)
        });
        Ok((files?, order))
    }
}

fn reverse_mapping(mapping: &[usize], n: usize) -> Vec<usize> {
    let mut res = vec![0; n];
    mapping.iter().enumerate().for_each(|(i, &x)| res[x] = i);
    res
}