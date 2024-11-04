use std::ops::Deref;

use crate::backend::{AttributeOp, Backend, DataContainer, DatasetOp, GroupOp};
use crate::data::array::{
    slice::{SelectInfoElem, Shape},
    CategoricalArray, DynArray, DynScalar,
};
use crate::data::data_traits::*;
use crate::data::index::{Index, Interval};

use anyhow::{bail, Result};
use log::warn;
use ndarray::{Array1, Array2};
use polars::chunked_array::ChunkedArray;
use polars::datatypes::DataType;
use polars::prelude::{DataFrame, Series};

use super::{SelectInfoBounds, SelectInfoElemBounds};

impl WriteData for DataFrame {
    fn data_type(&self) -> crate::backend::DataType {
        crate::backend::DataType::DataFrame
    }
    fn write<B: Backend, G: GroupOp<B>>(
        &self,
        location: &G,
        name: &str,
    ) -> Result<DataContainer<B>> {
        let mut group = if location.exists(name)? {
            location.open_group(name)?
        } else {
            location.new_group(name)?
        };
        group.new_str_attr("encoding-type", "dataframe")?;
        group.new_str_attr("encoding-version", "0.2.0")?;

        let columns: Array1<String> = self
            .get_column_names()
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        group.new_array_attr("column-order", &columns)?;
        self.iter()
            .try_for_each(|x| x.write(&group, x.name()).map(|_| ()))?;

        let container = DataContainer::Group(group);

        // Create an index as the python anndata package enforce it. This is not used by this library
        DataFrameIndex::from(self.height()).overwrite(container)
    }

    fn overwrite<B: Backend>(&self, mut container: DataContainer<B>) -> Result<DataContainer<B>> {
        if let Ok(index_name) = container.get_str_attr("_index") {
            for obj in container.as_group()?.list()? {
                if obj != index_name {
                    container.as_group()?.delete(&obj)?;
                }
            }
            let n = self.height();
            if n != 0 && n != container.as_group()?.open_dataset(&index_name)?.shape()[0] {
                container = DataFrameIndex::from(self.height()).overwrite(container)?;
            }
        } else {
            for obj in container.as_group()?.list()? {
                container.as_group()?.delete(&obj)?;
            }
            container = DataFrameIndex::from(self.height()).overwrite(container)?;
        }

        let columns: Array1<String> = self
            .get_column_names()
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        container.new_array_attr("column-order", &columns)?;
        self.iter()
            .try_for_each(|x| x.write(container.as_group()?, x.name()).map(|_| ()))?;
        container.new_str_attr("encoding-type", "dataframe")?;
        container.new_str_attr("encoding-version", "0.2.0")?;

        Ok(container)
    }
}

impl ReadData for DataFrame {
    fn read<B: Backend>(container: &DataContainer<B>) -> Result<Self> {
        let columns: Array1<String> = container.get_array_attr("column-order")?;
        columns
            .into_iter()
            .map(|x| {
                let name = x.as_str();
                let series_container = DataContainer::<B>::open(container.as_group()?, name)?;
                let mut series = Series::read::<B>(&series_container)?;
                series.rename(name.into());
                Ok(series)
            })
            .collect()
    }
}

impl HasShape for DataFrame {
    fn shape(&self) -> Shape {
        self.shape().into()
    }
}

impl ArrayOp for DataFrame {
    fn get(&self, _index: &[usize]) -> Option<DynScalar> {
        todo!()
    }

    fn select<S>(&self, info: &[S]) -> Self
    where
        S: AsRef<SelectInfoElem>,
    {
        if info.as_ref().len() != 2 {
            panic!("DataFrame only support 2D selection");
        }
        let columns = self.get_column_names();
        let select = SelectInfoBounds::new(&info, &HasShape::shape(self));
        let ridx = select.as_ref()[0].iter().map(|x| x as u32).collect();
        self.select(
            select.as_ref()[1]
                .to_vec()
                .into_iter()
                .map(|i| columns[i].as_str()),
        )
        .unwrap()
        .take(&ChunkedArray::from_vec("idx".into(), ridx))
        .unwrap()
    }

    fn vstack<I: Iterator<Item = Self>>(iter: I) -> Result<Self> {
        Ok(iter
            .reduce(|mut a, b| {
                a.vstack_mut(&b).unwrap();
                a
            })
            .unwrap_or(Self::empty()))
    }
}

impl ReadArrayData for DataFrame {
    fn get_shape<B: Backend>(container: &DataContainer<B>) -> Result<Shape> {
        let group = container.as_group()?;
        let index = group.get_str_attr("_index")?;
        let nrows = group.open_dataset(&index)?.shape()[0];
        let columns: Array1<String> = container.get_array_attr("column-order")?;
        Ok((nrows, columns.len()).into())
    }

    fn read_select<B, S>(container: &DataContainer<B>, info: &[S]) -> Result<Self>
    where
        B: Backend,
        S: AsRef<SelectInfoElem>,
    {
        let columns: Vec<String> = container.get_array_attr("column-order")?.to_vec();
        SelectInfoElemBounds::new(&info.as_ref()[1], columns.len())
            .iter()
            .map(|i| {
                let name = &columns[i];
                let mut series = container
                    .as_group()?
                    .open_dataset(name)
                    .map(DataContainer::Dataset)
                    .and_then(|x| Series::read_select::<B, _>(&x, &info[..1]))?;
                series.rename(name.into());
                Ok(series)
            })
            .collect()
    }
}

impl WriteArrayData for DataFrame {}

impl WriteData for Series {
    fn data_type(&self) -> crate::backend::DataType {
        crate::backend::DataType::DataFrame
    }
    fn write<B: Backend, G: GroupOp<B>>(
        &self,
        location: &G,
        name: &str,
    ) -> Result<DataContainer<B>> {
        match self.dtype() {
            DataType::UInt8 => self
                .u8()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::UInt16 => self
                .u16()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::UInt32 => self
                .u32()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::UInt64 => self
                .u64()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Int8 => self
                .i8()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Int16 => self
                .i16()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Int32 => self
                .i32()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Int64 => self
                .i64()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Float32 => self
                .f32()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Float64 => self
                .f64()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Boolean => self
                .bool()?
                .into_iter()
                .map(|x| x.unwrap())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::String => self
                .str()?
                .into_iter()
                .map(|x| x.unwrap().to_string())
                .collect::<Array1<_>>()
                .write(location, name),
            DataType::Categorical(_, _) => self
                .categorical()?
                .iter_str()
                .map(|x| x.unwrap())
                .collect::<CategoricalArray>()
                .write(location, name),
            other => bail!("Unsupported series data type: {:?}", other),
        }
    }
}

impl ReadData for Series {
    fn read<B: Backend>(container: &DataContainer<B>) -> Result<Self> {
        match container.encoding_type()? {
            crate::backend::DataType::Categorical => Ok(CategoricalArray::read(container)?.into()),
            crate::backend::DataType::Array(_) => Ok(DynArray::read(container)?.into()),
            ty => bail!("Unsupported data type: {:?}", ty),
        }
    }
}

impl HasShape for Series {
    fn shape(&self) -> Shape {
        self.len().into()
    }
}

impl ArrayOp for Series {
    fn get(&self, _index: &[usize]) -> Option<DynScalar> {
        todo!()
    }

    fn select<S>(&self, info: &[S]) -> Self
    where
        S: AsRef<SelectInfoElem>,
    {
        let i = SelectInfoElemBounds::new(info.as_ref()[0].as_ref(), self.len())
            .iter()
            .map(|x| x as u32)
            .collect::<Vec<_>>();
        self.take(&ChunkedArray::from_vec("idx".into(), i)).unwrap()
    }

    fn vstack<I: Iterator<Item = Self>>(_iter: I) -> Result<Self> {
        todo!("vstack not implemented for Series")
    }
}

impl ReadArrayData for Series {
    fn get_shape<B: Backend>(container: &DataContainer<B>) -> Result<Shape> {
        Ok(container.as_dataset()?.shape().into())
    }

    fn read_select<B, S>(container: &DataContainer<B>, info: &[S]) -> Result<Self>
    where
        B: Backend,
        S: AsRef<SelectInfoElem>,
    {
        Ok(Self::read(container)?.select(info))
    }
}

#[derive(Debug, Clone)]
pub struct DataFrameIndex {
    pub index_name: String,
    index: Index,
}

impl std::cmp::PartialEq for DataFrameIndex {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl DataFrameIndex {
    pub fn empty() -> Self {
        Self {
            index_name: "index".to_string(),
            index: Index::empty(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn get_index(&self, k: &str) -> Option<usize> {
        self.index.get_index(k)
    }

    pub fn into_vec(self) -> Vec<String> {
        self.index.into_vec()
    }

    pub fn select(&self, select: &SelectInfoElem) -> Self {
        let index = self.index.select(select);
        Self {
            index_name: self.index_name.clone(),
            index,
        }
    }
}

impl IntoIterator for DataFrameIndex {
    type Item = String;
    type IntoIter = Box<dyn Iterator<Item = String>>;

    fn into_iter(self) -> Self::IntoIter {
        self.index.into_iter()
    }
}

impl WriteData for DataFrameIndex {
    fn data_type(&self) -> crate::backend::DataType {
        crate::backend::DataType::DataFrame
    }
    fn write<B: Backend, G: GroupOp<B>>(
        &self,
        location: &G,
        name: &str,
    ) -> Result<DataContainer<B>> {
        let group = if location.exists(name)? {
            location.open_group(name)?
        } else {
            location.new_group(name)?
        };
        self.overwrite(DataContainer::Group(group))
    }

    fn overwrite<B: Backend>(&self, mut container: DataContainer<B>) -> Result<DataContainer<B>> {
        if let Ok(index_name) = container.get_str_attr("_index") {
            container.as_group()?.delete(&index_name)?;
        }
        container.new_str_attr("_index", &self.index_name)?;
        let group = container.as_group()?;
        let arr: Array1<String> = self.clone().into_iter().collect();
        let mut data = group.new_array_dataset(&self.index_name, arr.into(), Default::default())?;
        match &self.index {
            Index::List(_) => {
                data.new_str_attr("index_type", "list")?;
            }
            Index::Intervals(intervals) => {
                let keys: Array1<String> = intervals.keys().cloned().collect();
                let vec: Vec<u64> = intervals
                    .values()
                    .flat_map(|x| [x.start as u64, x.end as u64, x.size as u64, x.step as u64])
                    .collect();
                let values = Array2::from_shape_vec((intervals.deref().len(), 4), vec)?;
                if data.new_array_attr("names", &keys).is_err()
                    || data.new_array_attr("intervals", &values).is_err()
                {
                    // fallback to "list"
                    data.new_str_attr("index_type", "list")?;
                    warn!("Failed to save interval index as attributes, fallback to list index");
                } else {
                    data.new_str_attr("index_type", "intervals")?;
                }
            }
            Index::Range(range) => {
                data.new_str_attr("index_type", "range")?;
                data.new_scalar_attr("start", range.start as u64)?;
                data.new_scalar_attr("end", range.end as u64)?;
            }
        }
        Ok(container)
    }
}

impl ReadData for DataFrameIndex {
    fn read<B: Backend>(container: &DataContainer<B>) -> Result<Self> {
        let index_name = container.get_str_attr("_index")?;
        let dataset = container.as_group()?.open_dataset(&index_name)?;
        match dataset
            .get_str_attr("index_type")
            .as_ref()
            .map_or("list", |x| x.as_str())
        {
            "list" => {
                let data = dataset.read_array()?;
                let mut index: DataFrameIndex = data.to_vec().into();
                index.index_name = index_name;
                Ok(index)
            }
            "intervals" => {
                let keys: Array1<String> = dataset.get_array_attr("names")?;
                let values: Array2<u64> = dataset.get_array_attr("intervals")?;
                Ok(keys
                    .into_iter()
                    .zip(values.rows().into_iter().map(|row| Interval {
                        start: row[0] as usize,
                        end: row[1] as usize,
                        size: row[2] as usize,
                        step: row[3] as usize,
                    }))
                    .collect())
            }
            "range" => {
                let start: u64 = dataset.get_scalar_attr("start")?;
                let end: u64 = dataset.get_scalar_attr("end")?;
                Ok((start as usize..end as usize).into())
            }
            x => bail!("Unknown index type: {}", x),
        }
    }
}

impl<D> From<D> for DataFrameIndex
where
    Index: From<D>,
{
    fn from(data: D) -> Self {
        Self {
            index_name: "index".to_owned(),
            index: data.into(),
        }
    }
}

impl<D> FromIterator<D> for DataFrameIndex
where
    Index: FromIterator<D>,
{
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = D>,
    {
        Self {
            index_name: "index".to_owned(),
            index: iter.into_iter().collect(),
        }
    }
}
