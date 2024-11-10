from anndata_rs import AnnData, read, concat

import pytest
import anndata as ad
import numpy as np
import pandas as pd
import polars as pl
from pathlib import Path
import uuid
from scipy.sparse import csr_matrix, csc_matrix

def h5ad(dir=Path("./")):
    dir.mkdir(exist_ok=True)
    return str(dir / Path(str(uuid.uuid4()) + ".h5ad"))

@pytest.mark.parametrize("backend", ["hdf5", "zarr"])
def test_concat(tmp_path, backend):
    adata1 = AnnData(filename = h5ad(tmp_path), backend=backend)
    adata2 = AnnData(filename = h5ad(tmp_path), backend=backend)
    adata3 = AnnData(filename = h5ad(tmp_path), backend=backend)

    x1 = np.array([
        [1, 2, 3],
        [4, 5, 6],
        [7, 8, 9],
    ])
    x2 = np.array([
        [1, 2],
        [4, 5],
        [7, 8],
    ])
    x3 = np.array([
        [10, 20],
        [40, 50],
        [70, 80],
        [1, 1],
    ])
    x_merged = np.array([
        [1, 2, 3, 0, 0, 0],
        [4, 5, 6, 0, 0, 0],
        [7, 8, 9, 0, 0, 0],
        [0, 1, 0, 2, 0, 0],
        [0, 4, 0, 5, 0, 0],
        [0, 7, 0, 8, 0, 0],
        [0, 0, 0, 0, 10, 20],
        [0, 0, 0, 0, 40, 50],
        [0, 0, 0, 0, 70, 80],
        [0, 0, 0, 0, 1, 1],
    ])

    adata1.X = x1
    adata1.obs_names = ["1", "2", "3"]
    adata1.var_names = ["a", "b", "c"]

    adata2.X = x2
    adata2.obs_names = ["1", "2", "3"]
    adata2.var_names = ["b", "d"]

    adata3.X = x3
    adata3.obs_names = ["1", "2", "3", "4"]
    adata3.var_names = ["e", "f"]

    out = h5ad(tmp_path)
    merged = concat([adata1, adata2, adata3], join='outer', file=out)
    assert merged.obs_names == ["1", "2", "3", "1", "2", "3", "1", "2", "3", "4"]
    assert merged.var_names == ["a", "b", "c", "d", "e", "f"]
    np.testing.assert_array_equal(merged.X[:], x_merged)

    adata1.X = csr_matrix(x1)
    adata2.X = csr_matrix(x2)
    adata3.X = csr_matrix(x3)
    out = h5ad(tmp_path)
    merged = concat([adata1, adata2, adata3], join='outer', file=out)
    assert merged.obs_names == ["1", "2", "3", "1", "2", "3", "1", "2", "3", "4"]
    assert merged.var_names == ["a", "b", "c", "d", "e", "f"]
    np.testing.assert_array_equal(merged.X[:].todense(), x_merged)