from hypothesis import given, settings, HealthCheck, strategies as st
from hypothesis.extra.numpy import *
import pytest
from anndata_rs import AnnData, AnnDataSet

import numpy as np
import pandas as pd
from pathlib import Path
import uuid
from scipy import sparse as sp
from scipy.sparse import csr_matrix, issparse, random, vstack

def h5ad(dir=Path("./")):
    dir.mkdir(exist_ok=True)
    return str(dir / Path(str(uuid.uuid4()) + ".h5ad"))

@given(
    x = arrays(integer_dtypes(endianness='='), (47, 79)),
    obsm = arrays(integer_dtypes(endianness='='), (47, 139)),
    indices = st.lists(st.integers(min_value=0, max_value=46), min_size=0, max_size=50),
    obs = st.lists(st.integers(min_value=0, max_value=100000), min_size=47, max_size=47),
)
@settings(suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_subset2(x, obs, obsm, indices, tmp_path):
    csr = csr_matrix(obsm)
    adata = AnnData(
        X=x,
        obs = dict(txt=obs),
        obsm = dict(X_pca=obsm, sparse=csr),
        filename = h5ad(tmp_path),
    )
    adata.subset(indices)

    np.testing.assert_array_equal(adata.X[:], x[indices, :])
    np.testing.assert_array_equal(adata.obs["txt"], np.array(list(obs[i] for i in indices)))
    np.testing.assert_array_equal(adata.obsm["X_pca"], obsm[indices, :])
    np.testing.assert_array_equal(adata.obsm["sparse"].todense(), csr[indices, :].todense())

def test_subset(tmp_path):
    X = np.array([[1, 2, 3], [4, 5, 6], [7, 8, 9]])
    adata = AnnData(
        X=X,
        obs=dict(Obs=["A", "B", "C"]),
        var=dict(Feat=["a", "b", "c"]),
        obsm=dict(X_pca=np.array([[1, 2], [3, 4], [5, 6]])),
        filename=h5ad(tmp_path),
    )

    adata.subset([0, 1])

    np.testing.assert_array_equal(adata.obsm["X_pca"][...], np.array([[1, 2], [3, 4]]))

    X = random(5000, 50, 0.1, format="csr")
    adata = AnnData(
        X=X,
        obsm=dict(X_pca=X),
        filename=h5ad(tmp_path),
    )
    idx = np.random.randint(0, 5000, 1000)
    adata.subset(idx)
    np.testing.assert_array_equal(adata.X[...].todense(), X[idx,:].todense())
    np.testing.assert_array_equal(adata.obsm["X_pca"][...].todense(), X[idx,:].todense())

    X = random(5000, 50, 0.1, format="csr")
    adata = AnnData(
        X=X,
        obsm=dict(X_pca=X),
        filename=h5ad(tmp_path),
    )
    idx = np.random.choice([True, False], size=5000, p=[0.5, 0.5])
    adata.subset(idx)
    np.testing.assert_array_equal(adata.X[...].todense(), X[idx,:].todense())
    np.testing.assert_array_equal(adata.obsm["X_pca"][...].todense(), X[idx,:].todense())

def test_chunk(tmp_path):
    X = random(5000, 50, 0.1, format="csr", dtype=np.int64)
    adata = AnnData(
        X=X,
        filename=h5ad(tmp_path),
    )
    s = X.sum(axis = 0)
    s_ = np.zeros_like(s)
    for m in adata.X.chunked(47):
        s_ += m.sum(axis = 0)
    np.testing.assert_array_equal(s, s_)
    s_ = np.zeros_like(s)
    for m in adata.X.chunked(500000):
        s_ += m.sum(axis = 0)
    np.testing.assert_array_equal(s, s_)

    x1 = random(2321, 50, 0.1, format="csr", dtype=np.int64)
    x2 = random(2921, 50, 0.1, format="csr", dtype=np.int64)
    x3 = random(1340, 50, 0.1, format="csr", dtype=np.int64)
    merged = vstack([x1, x2, x3])
    adata1 = AnnData(X=x1, filename=h5ad(tmp_path))
    adata2 = AnnData(X=x2, filename=h5ad(tmp_path))
    adata3 = AnnData(X=x3, filename=h5ad(tmp_path))
    adata = AnnDataSet(
        [("1", adata1), ("2", adata2), ("3", adata3)],
        h5ad(tmp_path),
        "batch"
    )

    s = merged.sum(axis = 0)
    s_ = np.zeros_like(s)
    for m in adata.X.chunked(47):
        s_ += m.sum(axis = 0)
    np.testing.assert_array_equal(s, s_)
    s_ = np.zeros_like(s)
    for m in adata.X.chunked(500000):
        s_ += m.sum(axis = 0)
    np.testing.assert_array_equal(s, s_)

def test_anndataset(tmp_path):
    data1 = np.random.randint(10000, size=(15, 500))
    data2 = np.random.randint(10000, size=(99, 500))
    data3 = np.random.randint(10000, size=(87, 500))
    merged = np.concatenate([data1, data2, data3], axis=0)

    adata1 = AnnData(X=data1, filename=h5ad(tmp_path))
    adata2 = AnnData(X=data2, filename=h5ad(tmp_path))
    adata3 = AnnData(X=data3, filename=h5ad(tmp_path))

    dataset = AnnDataSet(
        [("1", adata1), ("2", adata2), ("3", adata3)],
        h5ad(tmp_path),
        "batch"
    )

    idx = np.random.randint(0, merged.shape[0], 50)
    np.testing.assert_array_equal(merged, dataset.X[:])
    np.testing.assert_array_equal(merged[idx, :], dataset.X[list(idx)])

    data1 = random(2000, 500, 0.01, format="csr")
    data2 = random(2231, 500, 0.2, format="csr")
    data3 = random(231, 500, 0.5, format="csr")
    merged = vstack([data1, data2, data3])
    adata1 = AnnData(X=data1, filename=h5ad(tmp_path))
    adata2 = AnnData(X=data2, filename=h5ad(tmp_path))
    adata3 = AnnData(X=data3, filename=h5ad(tmp_path))
    dataset = AnnDataSet(
        [("1", adata1), ("2", adata2), ("3", adata3)],
        h5ad(tmp_path),
        "batch"
    )
    idx = np.random.randint(0, merged.shape[0], 1193)
    np.testing.assert_array_equal(merged[idx, :].todense(), dataset.X[list(idx)].todense())

@given(
    x1 = arrays(np.int64, (15, 179)),
    x2 = arrays(np.int64, (47, 179)),
    x3 = arrays(np.int64, (77, 179)),
    idx1 = st.lists(st.integers(min_value=0, max_value=14), min_size=0, max_size=50),
    idx2 = st.lists(st.integers(min_value=15, max_value=15+46), min_size=0, max_size=50),
    idx3 = st.lists(st.integers(min_value=15+47, max_value=15+47+76), min_size=0, max_size=50),
)
@settings(suppress_health_check=[HealthCheck.function_scoped_fixture])
def test_anndataset_subset(x1, x2, x3, idx1, idx2, idx3, tmp_path):
    adata1 = AnnData(X=x1, filename=h5ad(tmp_path))
    adata2 = AnnData(X=x2, filename=h5ad(tmp_path))
    adata3 = AnnData(X=x3, filename=h5ad(tmp_path))
    merged = np.concatenate([x1, x2, x3], axis=0)
    indices = idx1 + idx2 + idx3

    dataset = AnnDataSet(
        [("1", adata1), ("2", adata2), ("3", adata3)], h5ad(tmp_path), "batch"
    )

    dataset.subset(indices)
    np.testing.assert_array_equal(merged[indices, :], dataset.X[:])