use super::{isinstance_of_pandas, IntoPython};

use arrow::ffi;
use polars::prelude::*;
use polars_arrow::export::arrow;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::{ffi::Py_uintptr_t, PyAny, PyObject, PyResult};

pub struct PyDataFrame(DataFrame);

impl From<DataFrame> for PyDataFrame {
    fn from(value: DataFrame) -> Self {
        PyDataFrame(value)
    }
}

impl From<PyDataFrame> for DataFrame {
    fn from(value: PyDataFrame) -> Self {
        value.0
    }
}

impl<'py> FromPyObject<'py> for PyDataFrame {
    fn extract(ob: &'py PyAny) -> PyResult<Self> {
        let py = ob.py();
        let df = if isinstance_of_pandas(py, ob)? {
            py.import("polars")?.call_method1("from_pandas", (ob, ))?
        } else if ob.is_instance_of::<pyo3::types::PyDict>()? {
            py.import("polars")?.call_method1("from_dict", (ob, ))?
        } else {
            ob
        };
        Ok(to_rust_df(ob.py(), df)?.into())
    }
}

impl IntoPy<PyObject> for PyDataFrame {
    fn into_py(self, py: Python<'_>) -> PyObject {
        to_py_df(py, self.0).unwrap()
    }
}

pub struct PySeries(Series);

impl From<Series> for PySeries {
    fn from(value: Series) -> Self {
        PySeries(value)
    }
}

impl From<PySeries> for Series {
    fn from(value: PySeries) -> Self {
        value.0
    }
}

impl<'py> FromPyObject<'py> for PySeries {
    fn extract(ob: &'py PyAny) -> PyResult<Self> {
        to_rust_series(ob).map(Into::into)
    }
}

impl IntoPython for &Series {
    fn into_python(self, py: Python) -> PyResult<PyObject> {
        to_py_series(py, self)
    }
}

/// Take an arrow array from python and convert it to a rust arrow array.
/// This operation does not copy data.
fn array_to_rust(arrow_array: &PyAny) -> PyResult<ArrayRef> {
    // prepare a pointer to receive the Array struct
    let array = Box::new(ffi::ArrowArray::empty());
    let schema = Box::new(ffi::ArrowSchema::empty());

    let array_ptr = &*array as *const ffi::ArrowArray;
    let schema_ptr = &*schema as *const ffi::ArrowSchema;

    // make the conversion through PyArrow's private API
    // this changes the pointer's memory and is thus unsafe. In particular, `_export_to_c` can go out of bounds
    arrow_array.call_method1(
        "_export_to_c",
        (array_ptr as Py_uintptr_t, schema_ptr as Py_uintptr_t),
    )?;

    unsafe {
        let field = ffi::import_field_from_c(schema.as_ref()).unwrap();
        let array = ffi::import_array_from_c(*array, field.data_type).unwrap();
        Ok(array.into())
    }
}

/// Arrow array to Python.
fn to_py_array(py: Python, pyarrow: &PyModule, array: ArrayRef) -> PyResult<PyObject> {
    let schema = Box::new(ffi::export_field_to_c(&ArrowField::new(
        "",
        array.data_type().clone(),
        true,
    )));
    let array = Box::new(ffi::export_array_to_c(array));

    let schema_ptr: *const ffi::ArrowSchema = &*schema;
    let array_ptr: *const ffi::ArrowArray = &*array;

    let array = pyarrow.getattr("Array")?.call_method1(
        "_import_from_c",
        (array_ptr as Py_uintptr_t, schema_ptr as Py_uintptr_t),
    )?;

    Ok(array.to_object(py))
}

fn to_rust_series(series: &PyAny) -> PyResult<Series> {
    // rechunk series so that they have a single arrow array
    let series = series.call_method0("rechunk")?;

    let name = series.getattr("name")?.extract::<String>()?;

    // retrieve pyarrow array
    let array = series.call_method0("to_arrow")?;

    // retrieve rust arrow array
    let array = array_to_rust(array)?;

    Series::try_from((name.as_str(), array)).map_err(|e| PyValueError::new_err(format!("{}", e)))
}

fn to_py_series<'py>(py: Python<'py>, series: &Series) -> PyResult<PyObject> {
    // ensure we have a single chunk
    let series = series.rechunk();
    let array = series.to_arrow(0);

    // import pyarrow
    let pyarrow = py.import("pyarrow")?;
    let pyarrow_array = to_py_array(py, pyarrow, array)?;

    // import polars
    let polars = py.import("polars")?;
    let out = polars.call_method1("from_arrow", (pyarrow_array,))?;
    Ok(out.to_object(py))
}

fn to_py_df<'py>(py: Python<'py>, df: DataFrame) -> PyResult<PyObject> {
    let pyarrow = py.import("pyarrow")?;

    let py_arrays: Vec<_> = df
        .iter()
        .map(|series| {
            let series = series.rechunk();
            let array = series.to_arrow(0);
            to_py_array(py, pyarrow, array).unwrap()
        })
        .collect();
    let arrow = pyarrow
        .getattr("Table")?
        .call_method1("from_arrays", (py_arrays, df.get_column_names()))?;
    let polars = py.import("polars")?;
    let df = polars.call_method1("from_arrow", (arrow,))?;
    Ok(df.to_object(py))
}

fn to_rust_df<'py>(py: Python<'py>, pydf: &PyAny) -> PyResult<DataFrame> {
    let series: Vec<_> = py
        .import("builtins")?
        .call_method1("list", (pydf,))?
        .extract()?;
    Ok(DataFrame::new(
        series
            .into_iter()
            .map(|x| to_rust_series(x).unwrap())
            .collect(),
    )
    .unwrap())
}